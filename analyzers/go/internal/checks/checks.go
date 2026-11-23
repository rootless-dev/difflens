// Package checks runs the static checkers of Go (go vet, and staticcheck
// when it is installed) on a source tree and reports their findings as
// analyzers/protocol/checks.schema.json describes. Checkers read the code
// and never write inside the tree: modules are used read-only and their
// caches live outside it.
package checks

import (
	"cmp"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/gocmd"
	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// Tool names, in the order they are reported.
const (
	ToolVet         = "go vet"
	ToolStaticcheck = "staticcheck"
)

// Finder locates the executable of an optional checker by name, or says why
// it is not installed.
type Finder func(ctx context.Context, name string) (string, error)

// Runner runs the checkers.
type Runner struct {
	log  logging.Logger
	find Finder
}

// New returns a runner that locates optional checkers with find.
func New(log logging.Logger, find Finder) *Runner {
	return &Runner{log: log, find: find}
}

// DefaultFinder looks in PATH, then in the bin directory of every GOPATH
// entry, where `go install` puts tools.
func DefaultFinder(ctx context.Context, name string) (string, error) {
	if path, err := exec.LookPath(name); err == nil {
		return path, nil
	}
	values, err := gocmd.GoEnv(ctx, "", os.Environ(), "GOPATH", "GOEXE")
	if err == nil {
		for _, dir := range filepath.SplitList(values["GOPATH"]) {
			candidate := filepath.Join(dir, "bin", name+values["GOEXE"])
			if info, err := os.Stat(candidate); err == nil && !info.IsDir() {
				return candidate, nil
			}
		}
	}
	return "", fmt.Errorf("%s is not installed (not in PATH nor in the GOPATH bin directory)", name)
}

// Result is the outcome of every checker, in a fixed order.
type Result struct {
	Checks      []protocol.Check
	Diagnostics []string
}

// Run runs every checker on the tree at dir within ctx. cacheDir is the
// core's persistent folder for the project (--cache-dir), or "": the
// checkers use the user's build cache, and one in cacheDir only when the
// user's is not usable (see gocmd.BuildCache).
func (r *Runner) Run(ctx context.Context, dir, cacheDir string) (*Result, error) {
	tree, units, err := analyzer.OpenTree(dir)
	if err != nil {
		return nil, err
	}
	res := &Result{Diagnostics: []string{}}
	env := r.baseEnv(ctx, units[0], cacheDir)
	vet := newCheck(ToolVet)
	for _, unit := range units {
		r.vet(ctx, tree, unit, env, vet, res)
	}
	res.Checks = append(res.Checks, vet.result())

	sc := newCheck(ToolStaticcheck)
	if path, err := r.find(ctx, ToolStaticcheck); err != nil {
		sc.unavailable(err.Error())
	} else {
		for _, unit := range units {
			r.staticcheck(ctx, tree, unit, env, path, sc)
		}
	}
	res.Checks = append(res.Checks, sc.result())
	if ctx.Err() != nil {
		return nil, fmt.Errorf("checks did not finish in time: %w", ctx.Err())
	}
	return res, nil
}

// baseEnv is the environment the checkers start from: the analyzer's own,
// with GOCACHE in cacheDir when the user's build cache is not usable.
func (r *Runner) baseEnv(ctx context.Context, unit loader.Unit, cacheDir string) []string {
	env := os.Environ()
	if cacheDir == "" {
		return env
	}
	cache, own, err := gocmd.BuildCache(ctx, unit.Dir, unit.Env(env, ""), cacheDir)
	switch {
	case err != nil:
		r.log.Warn("build cache", "error", err.Error())
	case own:
		r.log.Info("the user's build cache is not usable: using one in --cache-dir", "GOCACHE", cache)
		env = gocmd.SetEnv(env, "GOCACHE", cache)
	}
	return env
}

// check accumulates one checker's outcome over the units of the tree.
type check struct {
	tool     string
	findings []protocol.Finding
	failures []string // why it could not check some code
	missing  string   // set when not installed
}

func newCheck(tool string) *check { return &check{tool: tool} }

func (c *check) unavailable(why string) { c.missing = why }

func (c *check) fail(why string) {
	if why != "" && !slices.Contains(c.failures, why) {
		c.failures = append(c.failures, why)
	}
}

func (c *check) add(f protocol.Finding) { c.findings = append(c.findings, f) }

// result is the protocol entry. Findings of the code that could be checked
// are kept when another part failed.
func (c *check) result() protocol.Check {
	out := protocol.Check{Tool: c.tool, Findings: dedupeFindings(c.findings)}
	switch {
	case c.missing != "":
		out.Status, out.Message = protocol.CheckUnavailable, ptr(c.missing)
		out.Findings = []protocol.Finding{}
	case len(c.failures) == 1:
		out.Status, out.Message = protocol.CheckFailed, ptr(c.failures[0])
	case len(c.failures) > 1:
		out.Status = protocol.CheckFailed
		out.Message = ptr(fmt.Sprintf("%s (and %d more problems)", c.failures[0], len(c.failures)-1))
	case len(out.Findings) > 0:
		out.Status = protocol.CheckFindings
	default:
		out.Status = protocol.CheckClean
	}
	return out
}

func dedupeFindings(findings []protocol.Finding) []protocol.Finding {
	compare := func(a, b protocol.Finding) int {
		return cmp.Or(
			cmp.Compare(a.File, b.File), cmp.Compare(a.Line, b.Line),
			cmp.Compare(deref(a.Rule), deref(b.Rule)), cmp.Compare(a.Message, b.Message),
		)
	}
	slices.SortFunc(findings, compare)
	findings = slices.CompactFunc(findings, func(a, b protocol.Finding) bool { return compare(a, b) == 0 })
	if findings == nil {
		return []protocol.Finding{}
	}
	return findings
}

// position splits "file:line[:col]" from the right, so that a Windows drive
// letter stays in the file name.
func position(posn string) (file string, line int, ok bool) {
	i := strings.LastIndex(posn, ":")
	if i < 0 {
		return "", 0, false
	}
	last, err := strconv.Atoi(posn[i+1:])
	if err != nil {
		return "", 0, false
	}
	head := posn[:i]
	// With a column, the line is the field before it.
	if j := strings.LastIndex(head, ":"); j >= 0 {
		if l, err := strconv.Atoi(head[j+1:]); err == nil {
			return head[:j], l, true
		}
	}
	return head, last, true
}

// relFile returns file relative to the tree, or false when it lies outside.
func relFile(tree analyzer.Tree, unitDir, file string) (string, bool) {
	if file == "" {
		return "", false
	}
	if !filepath.IsAbs(file) {
		file = filepath.Join(unitDir, file)
	}
	return tree.Rel(file)
}

func ptr[T any](v T) *T { return &v }

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}
