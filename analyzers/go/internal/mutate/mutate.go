// Package mutate implements mutation testing of the lines a review adds, as
// analyzers/protocol/mutation.schema.json describes it: it changes one small
// piece of an added line that the tests execute (a mutant), runs the tests
// of the line's package against it, and reports whether one of them failed
// (killed) or none did (survived).
//
// The mutants are made by the analyzer's own mutator (Find) on the syntax
// and types go/packages loads for the packages holding candidate lines, and
// tested one at a time through `go test -overlay`, so the tree is never
// copied or written (see exec.go). Every package is first tested unmutated
// (its baseline), which also warms the build cache; a mutant is stopped
// after max(--min-mutant-timeout, 3 x its package's baseline). Up to --jobs
// mutants are tested at the same time, each with its own overlay and test
// binary. Results are kept in --cache-dir between runs (see cache.go).
//
// Function ids are the ids analyze writes (funcindex): the innermost
// function holding the mutant, closures included, since analyze lists
// closures as functions of their own.
package mutate

import (
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"go/ast"
	"go/token"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"golang.org/x/tools/go/packages"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/funcindex"
	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
	"github.com/rootless-dev/difflens/analyzers/go/internal/testrun"
)

// EngineName is the name the document gives the mutator.
const EngineName = "difflens-analyzer-go mutator"

// Defaults of the command line.
const (
	DefaultMaxMutants       = 200
	DefaultMinMutantTimeout = 10 * time.Second
)

// DefaultJobs is the number of mutants tested at the same time when --jobs
// is not given: half the CPUs, at least one.
func DefaultJobs() int { return max(1, runtime.NumCPU()/2) }

// Options of a mutation run.
type Options struct {
	Dir     string
	Timeout time.Duration // the whole run, baselines included
	// Added holds the lines the review adds, per file relative to Dir:
	// inclusive [start, end] ranges.
	Added    map[string][][2]int
	Coverage []protocol.CoverageBlock
	// CacheDir is the core's persistent folder for this project, or "": the
	// build cache when the user's is not usable, and the results kept.
	CacheDir         string
	MaxMutants       int
	Jobs             int // 0 for DefaultJobs
	MinMutantTimeout time.Duration
	Progress         string // file to keep up to date, "" for none
	Offline          bool
	// Engine is the mutator's name and version, part of every fingerprint.
	Engine protocol.Analyzer
}

// Result is a mutation run, without the runner's identity.
type Result struct {
	Mutants     []protocol.Mutant
	Summary     protocol.MutationSummary
	Diagnostics []string
	// Tested counts the mutants whose tests ran in this run (not reused).
	Tested int
}

// Runner runs mutation tests.
type Runner struct {
	log   logging.Logger
	tests *testrun.Runner
}

// New returns a runner reporting progress to log, building the go command's
// environment as tests does.
func New(log logging.Logger, tests *testrun.Runner) *Runner {
	return &Runner{log: log, tests: tests}
}

// Run makes the mutants of the candidate lines of the tree at opts.Dir and
// tests them. It fails only when the tree cannot be opened; packages that
// do not load, tests that fail on the unmutated code and a run stopped by
// the timeout are diagnostics of the result.
func (r *Runner) Run(ctx context.Context, opts Options) (*Result, error) {
	start := time.Now()
	tree, units, err := analyzer.OpenTree(opts.Dir)
	if err != nil {
		return nil, err
	}
	tmp, err := os.MkdirTemp("", "difflens-mutate-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(tmp)
	if opts.Jobs <= 0 {
		opts.Jobs = DefaultJobs()
	}

	runCtx, cancel := context.WithTimeout(ctx, opts.Timeout)
	defer cancel()
	s := &session{
		log: r.log, opts: opts, tree: tree, tmp: tmp, ids: map[string]bool{},
		store: newStore(opts.CacheDir), progress: newProgress(opts.Progress), phase: protocol.PhaseBaseline,
	}
	if lines := candidateLines(opts.Added, opts.Coverage); len(lines) > 0 {
		base := r.tests.BaseEnv(runCtx, units[0], opts.CacheDir)
		s.list(runCtx, units, base, lines)
	}
	candidates := len(s.mutants)
	var sampled bool
	s.mutants, sampled = sample(s.mutants, opts.MaxMutants)
	s.reuse()
	r.log.Info("mutants listed", "candidates", candidates, "sampled", sampled, "mutants", len(s.mutants), "cached", s.count(func(m *mutant) bool { return m.cached }))
	s.report()
	s.baselines(runCtx)
	s.testAll(runCtx)
	return s.result(candidates, sampled, time.Since(start)), nil
}

// session is the state of one Run.
type session struct {
	log      logging.Logger
	opts     Options
	tree     analyzer.Tree
	tmp      string
	store    store
	progress *progress
	mutants  []*mutant
	ids      map[string]bool

	mu          sync.Mutex // guards what follows, and the results of the mutants
	phase       string
	running     []*mutant
	diagnostics []string
	failures    []string // mutants go could not test, with why
	tested      int
}

// pkgInfo is a package holding mutants.
type pkgInfo struct {
	path  string // import path
	dir   string // absolute directory
	unit  loader.Unit
	env   []string
	arg   string // the package as go test names it in the unit's directory
	tests string // hash of its test files (cache.go)

	// Set by the baseline: the time limit of its mutants, or why they
	// cannot be tested and the status they get then.
	limit   time.Duration
	problem string
	status  string
}

// mutant is one mutation point of a candidate line with its result.
type mutant struct {
	Point
	pkg         *pkgInfo
	file        string // relative to the tree
	abs         string // absolute, as the go command names it
	src         []byte // the file's unmutated content
	function    *string
	group       groupKey
	line        int
	column      int
	endLine     int
	endColumn   int
	id          string
	fingerprint string

	status   string // "" until it has a result; left out when still "" at the end
	killedBy *string
	duration time.Duration
	cached   bool
	keep     bool // the result depends only on the code: keep it (rule 6)
}

func (s *session) diag(msg string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.diagnostics = append(s.diagnostics, msg)
}

// list loads the packages holding candidate lines, unit by unit, and makes
// their mutants, sorted by file and offset.
func (s *session) list(ctx context.Context, units []loader.Unit, base []string, lines map[string]map[int]bool) {
	files := make([]string, 0, len(lines))
	for file := range lines {
		files = append(files, file)
	}
	slices.Sort(files)
	byUnit := map[int][]string{}
	for _, file := range files {
		if u := unitOf(units, filepath.Join(s.tree.Root, filepath.FromSlash(file))); u >= 0 {
			byUnit[u] = append(byUnit[u], file)
		}
	}
	for i, unit := range units {
		if len(byUnit[i]) == 0 {
			continue
		}
		env := testrun.TestEnv(base, unit, s.opts.Offline)
		start := time.Now()
		pkgs, fset, err := loader.LoadPackages(ctx, unit, env, s.patterns(unit, byUnit[i]))
		if err != nil {
			s.log.Warn("load packages", "dir", unit.Dir, "error", err.Error())
			s.diag(fmt.Sprintf("the packages of %s could not be loaded, so the mutants of their lines were not made: %v", s.relDir(unit.Dir), err))
			continue
		}
		s.log.Info("packages loaded", "dir", unit.Dir, "packages", len(pkgs), "elapsed", time.Since(start).Round(time.Millisecond).String())
		for _, p := range pkgs {
			s.listPackage(p, fset, unit, env, lines)
		}
	}
	slices.SortStableFunc(s.mutants, func(a, b *mutant) int {
		return cmp.Or(strings.Compare(a.file, b.file), cmp.Compare(a.Start, b.Start))
	})
}

// unitOf returns the index of the unit holding file: the one with the
// deepest directory holding it, or -1.
func unitOf(units []loader.Unit, file string) int {
	best := -1
	for i, u := range units {
		rel, err := filepath.Rel(u.Dir, file)
		if err == nil && filepath.IsLocal(rel) && (best < 0 || len(u.Dir) > len(units[best].Dir)) {
			best = i
		}
	}
	return best
}

// patterns returns the package patterns of the directories of files, in
// the unit's directory.
func (s *session) patterns(unit loader.Unit, files []string) []string {
	var out []string
	for _, file := range files {
		dir := filepath.Dir(filepath.Join(s.tree.Root, filepath.FromSlash(file)))
		if p := packageArg(unit.Dir, dir); !slices.Contains(out, p) {
			out = append(out, p)
		}
	}
	return out
}

// packageArg names the package in dir for the go command run in unitDir:
// "." or "./<relative dir>"; "" when dir is outside unitDir.
func packageArg(unitDir, dir string) string {
	rel, err := filepath.Rel(unitDir, dir)
	switch {
	case err != nil || (!filepath.IsLocal(rel) && rel != "."):
		return ""
	case rel == ".":
		return "."
	}
	return "./" + filepath.ToSlash(rel)
}

func (s *session) relDir(dir string) string {
	if rel, ok := s.tree.Rel(dir); ok {
		return rel
	}
	return "."
}

// listPackage makes the mutants of the candidate lines of p's files. Test
// files are not loaded, and generated files and cgo files are skipped.
func (s *session) listPackage(p *packages.Package, fset *token.FileSet, unit loader.Unit, env []string, lines map[string]map[int]bool) {
	var pkg *pkgInfo
	for _, f := range p.Syntax {
		abs := fset.PositionFor(f.Pos(), false).Filename
		rel, ok := s.tree.Rel(abs)
		want := lines[rel]
		if !ok || want == nil || ast.IsGenerated(f) || importsC(f) {
			continue
		}
		tf := fset.File(f.Pos())
		src, err := os.ReadFile(abs)
		if err != nil || tf == nil || len(src) != tf.Size() {
			s.diag(fmt.Sprintf("%s could not be read as it was loaded, so its mutants were not made", rel))
			continue
		}
		if pkg == nil {
			dir := filepath.Dir(abs)
			pkg = &pkgInfo{path: p.PkgPath, dir: dir, unit: unit, env: env, arg: packageArg(unit.Dir, dir), tests: testsHash(dir)}
			if pkg.arg == "" {
				pkg.arg = p.PkgPath
			}
		}
		ix := funcindex.New(fset)
		ix.Add(funcindex.Package{Path: p.PkgPath, Name: p.Name, Files: []funcindex.File{{Syntax: f, RelPath: rel}}})
		lix := newLineIndex(src)
		for _, pt := range Find(fset, f, src, p.TypesInfo) {
			line, column := lix.position(pt.Start)
			if !want[line] {
				continue
			}
			m := &mutant{Point: pt, pkg: pkg, file: rel, abs: abs, src: src, line: line, column: column}
			m.endLine, m.endColumn = lix.position(pt.End)
			s.identify(m, ix, tf)
			if !s.ids[m.id] {
				s.ids[m.id] = true
				s.mutants = append(s.mutants, m)
			}
		}
	}
}

// identify gives m its function, description, group, id and fingerprint.
// The scope of a mutant is its innermost function that analyze lists, or
// else its top-level declaration; its position is its offset in the scope,
// so that edits elsewhere in the file keep its id and its kept result.
func (s *session) identify(m *mutant, ix *funcindex.Index, tf *token.File) {
	scope := ast.Node(m.Decl)
	var fn *funcindex.Func
	for _, n := range m.Scopes {
		key := token.NoPos
		switch n := n.(type) {
		case *ast.FuncDecl:
			key = n.Name.Pos()
		case *ast.FuncLit:
			key = n.Type.Func
		}
		if f, ok := ix.At(key); ok {
			scope, fn = n, f
			break
		}
	}
	start, end := tf.Offset(scope.Pos()), tf.Offset(scope.End())
	offset := strconv.Itoa(m.Start - start)
	funcID := ""
	if fn != nil {
		id := fn.ID
		funcID, m.function = id, &id
		m.group = groupKey{file: m.file, scope: start}
		m.Description += " in " + fn.Name
	} else {
		m.group = groupKey{file: m.file, scope: -1}
		m.Description += " at top level"
	}
	m.id = hashOf(m.file, funcID, m.Operator, m.Original, m.Mutated, offset)[:16]
	m.fingerprint = hashOf(s.opts.Engine.Name, s.opts.Engine.Version, m.file, funcID,
		string(m.src[start:end]), m.pkg.tests, m.Operator, m.Original, m.Mutated, offset)
}

// importsC reports whether f uses cgo, whose files the go command rewrites
// before compiling them: an overlay of the source would not apply.
func importsC(f *ast.File) bool {
	for _, imp := range f.Imports {
		if imp.Path.Value == `"C"` {
			return true
		}
	}
	return false
}

// reuse gives the mutants their kept results.
func (s *session) reuse() {
	for _, m := range s.mutants {
		if r, ok := s.store.get(m.fingerprint); ok {
			m.status, m.killedBy, m.duration, m.cached = r.Status, r.KilledBy, time.Duration(r.DurationMs)*time.Millisecond, true
		}
	}
}

// count counts the mutants for which keep is true.
func (s *session) count(keep func(*mutant) bool) int {
	n := 0
	for _, m := range s.mutants {
		if keep(m) {
			n++
		}
	}
	return n
}

// report writes the progress. The caller holds s.mu, or no worker runs.
func (s *session) report() {
	if s.progress == nil {
		return
	}
	doc := protocol.MutationProgress{Phase: s.phase, Total: len(s.mutants)}
	for _, m := range s.mutants {
		if m.status != "" {
			doc.Done++
		}
		switch m.status {
		case protocol.MutantKilled:
			doc.Killed++
		case protocol.MutantSurvived:
			doc.Survived++
		case protocol.MutantTimeout:
			doc.Timeout++
		case protocol.MutantError, protocol.MutantNoCoverage:
			doc.Error++
		}
	}
	current := ""
	if len(s.running) > 0 {
		current = s.running[0].file + ":" + strconv.Itoa(s.running[0].line)
	}
	s.progress.write(doc, current)
}

// parallel calls do(0) to do(n-1), up to opts.Jobs at the same time.
func (s *session) parallel(n int, do func(i int)) {
	next := make(chan int)
	var wg sync.WaitGroup
	for range min(s.opts.Jobs, n) {
		wg.Go(func() {
			for i := range next {
				do(i)
			}
		})
	}
	for i := range n {
		next <- i
	}
	close(next)
	wg.Wait()
}

// baselines runs the tests of every package with a mutant left to test,
// unmutated, and settles the mutants of the packages whose tests cannot
// tell anything: they do not build, they fail, or there are none.
func (s *session) baselines(ctx context.Context) {
	var pkgs []*pkgInfo
	for _, m := range s.mutants {
		if m.status == "" && !slices.Contains(pkgs, m.pkg) {
			pkgs = append(pkgs, m.pkg)
		}
	}
	if len(pkgs) == 0 {
		return
	}
	s.parallel(len(pkgs), func(i int) { s.baseline(ctx, pkgs[i]) })
	for _, pkg := range pkgs {
		if pkg.problem == "" {
			continue
		}
		n := 0
		for _, m := range s.mutants {
			if m.pkg == pkg && m.status == "" {
				m.status = pkg.status
				n++
			}
		}
		s.diag(fmt.Sprintf("%s, so its %d mutants were not tested", pkg.problem, n))
	}
}

func (s *session) baseline(ctx context.Context, pkg *pkgInfo) {
	dir, err := os.MkdirTemp(s.tmp, "baseline-")
	if err != nil {
		pkg.problem, pkg.status = fmt.Sprintf("the tests of %s could not be built: %v", pkg.path, err), protocol.MutantError
		return
	}
	defer os.RemoveAll(dir)
	start := time.Now()
	bin := filepath.Join(dir, testBinary)
	b, err := build(ctx, pkg, bin, "")
	switch {
	case ctx.Err() != nil:
		return // the time ran out: its mutants are left out
	case err != nil:
		pkg.problem, pkg.status = fmt.Sprintf("go test could not build the tests of %s: %v", pkg.path, err), protocol.MutantError
		return
	case b.noTests:
		pkg.problem, pkg.status = fmt.Sprintf("%s has no tests of its own (the coverage of its lines comes from the tests of other packages)", pkg.path), protocol.MutantNoCoverage
		return
	case !b.ok:
		pkg.problem, pkg.status = fmt.Sprintf("the tests of %s do not build without any mutant: %s", pkg.path, oneLine(b.output)), protocol.MutantError
		return
	}
	built := time.Since(start)
	res, err := run(ctx, pkg.dir, pkg.env, bin, 0)
	switch {
	case ctx.Err() != nil:
		return
	case err != nil:
		pkg.problem, pkg.status = fmt.Sprintf("the tests of %s could not run: %v", pkg.path, err), protocol.MutantError
		return
	case !res.passed:
		why := res.failed
		if why == "" {
			why = oneLine(res.output)
		}
		pkg.problem, pkg.status = fmt.Sprintf("the tests of %s fail without any mutant (%s)", pkg.path, why), protocol.MutantError
		return
	}
	pkg.limit = max(s.opts.MinMutantTimeout, 3*res.elapsed)
	s.log.Info("baseline done", "package", pkg.path, "build", built.Round(time.Millisecond).String(),
		"tests", res.elapsed.Round(time.Millisecond).String(), "limit", pkg.limit.String())
}

// testBinary is the name of a test binary.
var testBinary = func() string {
	if runtime.GOOS == "windows" {
		return "pkg.test.exe"
	}
	return "pkg.test"
}()

// oneLine returns the first line of out that says something.
func oneLine(out string) string {
	for line := range strings.SplitSeq(out, "\n") {
		if line = strings.TrimSpace(line); line != "" && !strings.HasPrefix(line, "# ") {
			return line
		}
	}
	return strings.TrimSpace(out)
}

// testAll tests the mutants left, up to opts.Jobs at the same time, in the
// order of the report.
func (s *session) testAll(ctx context.Context) {
	var todo []*mutant
	for _, m := range s.mutants {
		if m.status == "" && m.pkg.limit > 0 {
			todo = append(todo, m)
		}
	}
	s.mu.Lock()
	s.phase = protocol.PhaseMutating
	s.report()
	s.mu.Unlock()
	s.parallel(len(todo), func(i int) {
		m := todo[i]
		if ctx.Err() != nil {
			return
		}
		s.mu.Lock()
		s.running = append(s.running, m)
		s.report()
		s.mu.Unlock()

		o, err := s.test(ctx, m)

		s.mu.Lock()
		defer s.mu.Unlock()
		s.running = slices.DeleteFunc(s.running, func(r *mutant) bool { return r == m })
		switch {
		case ctx.Err() != nil:
			// The time ran out while it ran: it is left out.
		case err != nil:
			m.status = protocol.MutantError
			s.failures = append(s.failures, fmt.Sprintf("%s:%d: %v", m.file, m.line, err))
		default:
			m.status, m.killedBy, m.duration, m.keep = o.status, o.killedBy, o.duration, true
			s.tested++
		}
		s.log.Debug("mutant tested", "file", m.file, "line", m.line, "description", m.Description, "status", m.status)
		if m.keep {
			r := record{Status: m.status, KilledBy: m.killedBy, DurationMs: m.duration.Milliseconds()}
			if err := s.store.put(m.fingerprint, r); err != nil {
				s.log.Warn("keep a mutation result", "error", err.Error())
			}
		}
		s.report()
	})
}

// outcome is the result of testing one mutant.
type outcome struct {
	status   string
	killedBy *string
	duration time.Duration
}

// test builds the tests of m's package with m applied and runs them. A
// mutant that does not build is an error; the error return says that go
// could not test it at all.
func (s *session) test(ctx context.Context, m *mutant) (outcome, error) {
	dir, err := os.MkdirTemp(s.tmp, "mutant-")
	if err != nil {
		return outcome{}, err
	}
	defer os.RemoveAll(dir)
	mutated := filepath.Join(dir, filepath.Base(m.abs))
	if err := os.WriteFile(mutated, m.Apply(m.src), 0o644); err != nil {
		return outcome{}, err
	}
	overlay := filepath.Join(dir, "overlay.json")
	data, err := json.Marshal(map[string]map[string]string{"Replace": {m.abs: mutated}})
	if err != nil {
		return outcome{}, err
	}
	if err := os.WriteFile(overlay, data, 0o644); err != nil {
		return outcome{}, err
	}
	bin := filepath.Join(dir, testBinary)
	b, err := build(ctx, m.pkg, bin, overlay)
	if err != nil {
		return outcome{}, err
	}
	if !b.ok {
		if b.noTests {
			return outcome{}, errors.New("the package has no tests")
		}
		return outcome{status: protocol.MutantError}, nil
	}
	res, err := run(ctx, m.pkg.dir, m.pkg.env, bin, m.pkg.limit)
	switch {
	case err != nil:
		return outcome{}, err
	case res.timedOut:
		return outcome{status: protocol.MutantTimeout, duration: m.pkg.limit}, nil
	case res.passed:
		return outcome{status: protocol.MutantSurvived, duration: res.elapsed}, nil
	}
	o := outcome{status: protocol.MutantKilled, duration: res.elapsed}
	if res.failed != "" {
		o.killedBy = &res.failed
	}
	return o, nil
}

// result assembles the mutants with a result in the order of rule 7, and
// the summary.
func (s *session) result(candidates int, sampled bool, elapsed time.Duration) *Result {
	res := &Result{Mutants: []protocol.Mutant{}, Tested: s.tested}
	res.Summary = protocol.MutationSummary{Candidates: candidates, Sampled: sampled, MaxMutants: s.opts.MaxMutants}
	leftOut := 0
	for _, m := range s.mutants {
		if m.status == "" {
			leftOut++
			continue
		}
		res.Mutants = append(res.Mutants, protocol.Mutant{
			ID: m.id, File: m.file, Line: m.line, Column: m.column, EndLine: m.endLine, EndColumn: m.endColumn,
			Original: m.Original, Mutated: m.Mutated, Operator: m.Operator, Description: m.Description,
			Function: m.function, Status: m.status, KilledBy: m.killedBy, DurationMs: m.duration.Milliseconds(), Cached: m.cached,
		})
		switch m.status {
		case protocol.MutantKilled:
			res.Summary.Killed++
		case protocol.MutantSurvived:
			res.Summary.Survived++
		case protocol.MutantTimeout:
			res.Summary.Timeout++
		case protocol.MutantError:
			res.Summary.Error++
		case protocol.MutantNoCoverage:
			res.Summary.NoCoverage++
		}
		if m.cached {
			res.Summary.Cached++
		}
	}
	slices.SortStableFunc(res.Mutants, func(a, b protocol.Mutant) int {
		return cmp.Or(strings.Compare(a.File, b.File), cmp.Compare(a.Line, b.Line), cmp.Compare(a.Column, b.Column), strings.Compare(a.ID, b.ID))
	})
	diags := s.diagnostics
	if len(s.failures) > 0 {
		diags = append(diags, fmt.Sprintf("go could not test %d mutants, reported as errors; the first: %s", len(s.failures), s.failures[0]))
	}
	if leftOut > 0 {
		diags = append(diags, fmt.Sprintf("the run was stopped after %s: %d of the %d mutants were not tested and are left out", s.opts.Timeout, leftOut, len(s.mutants)))
	}
	res.Diagnostics = append([]string{}, diags...)
	res.Summary.DurationMs = elapsed.Milliseconds()
	return res
}
