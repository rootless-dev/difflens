// Package testrun runs the tests of a Go source tree once, with coverage and
// the race detector, and reports them as analyzers/protocol/tests.schema.json
// describes. It also prepares a tree so that the run can be offline.
package testrun

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/gocmd"
	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// Runner runs and prepares the tests of source trees.
type Runner struct {
	log logging.Logger
}

// New returns a runner reporting progress to log.
func New(log logging.Logger) *Runner { return &Runner{log: log} }

// Options of a test run.
type Options struct {
	Timeout time.Duration // the whole run, builds included
	Offline bool          // no network: the go command must not try to download
	Race    bool          // run with the race detector when the toolchain can
	Cover   bool          // measure statement coverage
	// Selection narrows the run to some packages and tests; nil runs them all.
	Selection *protocol.Selection
	// Progress is a file to keep up to date with how far the run got; "" for none.
	Progress string
	// CacheDir is the core's persistent folder for this project (--cache-dir),
	// or "". It holds the build cache only when the user's is not usable.
	CacheDir string
}

// Result is a test run, without the runner's identity.
type Result struct {
	Race        bool
	Tests       []protocol.Test
	Packages    []protocol.TestPackage
	Coverage    *protocol.Coverage
	Races       []protocol.Race
	Diagnostics []string
}

// maxStderr bounds what is kept of the go command's own stderr.
const maxStderr = 256 << 10

// Run runs the tests of the tree at dir once: all of them, or those of
// opts.Selection. It fails only when the tree cannot be run at all (no
// module); failing tests, packages that do not build and a run stopped by
// the timeout are reported in the result.
func (r *Runner) Run(ctx context.Context, dir string, opts Options) (*Result, error) {
	tree, units, err := analyzer.OpenTree(dir)
	if err != nil {
		return nil, err
	}
	tmp, err := os.MkdirTemp("", "difflens-test-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(tmp)

	runCtx, cancel := context.WithTimeout(ctx, opts.Timeout)
	defer cancel()
	s := &session{
		log: r.log, opts: opts, tmp: tmp, race: opts.Race,
		catalog: newCatalog(tree), cover: newCoverage(),
		result:   &Result{Race: opts.Race},
		progress: newProgress(opts.Progress),
	}
	if !opts.Race {
		s.diag("the race detector was off: --no-race was given")
	}
	if !opts.Cover {
		s.diag("coverage was not measured: --no-cover was given")
	}
	// Every unit is listed before any test runs, so that the progress knows
	// how many packages there are.
	found := map[string]bool{}
	plans := make([]unitPlan, 0, len(units))
	base := r.baseEnv(runCtx, units[0], opts.CacheDir)
	for _, unit := range units {
		p := unitPlan{unit: unit, rel: relDir(tree, unit.Dir), env: testEnv(base, unit, opts.Offline)}
		listed, err := s.catalog.list(runCtx, unit.Dir, p.env, unit.Patterns)
		if err != nil {
			s.log.Warn("list packages", "dir", unit.Dir, "error", err.Error())
			s.diag(fmt.Sprintf("could not list the packages of %s, so some tests are not linked to their functions: %v", p.rel, err))
		}
		var total int
		p.invocations, total = plan(unit.Patterns, listed, opts.Selection, s.catalog.container, found)
		s.total += total
		plans = append(plans, p)
	}
	if opts.Selection != nil {
		for _, p := range opts.Selection.Packages {
			if !found[p.Package] {
				s.diag(fmt.Sprintf("the package %s was not found, so its tests were not run", p.Package))
			}
		}
	}
	s.report(nil, true)
	for _, p := range plans {
		for _, inv := range p.invocations {
			if runCtx.Err() != nil {
				s.diag(fmt.Sprintf("the tests of %s were not run: the time ran out", p.rel))
				s.stopped = true
				break
			}
			s.runInvocation(runCtx, p, inv)
		}
	}
	s.report(nil, true)
	result := s.finish()
	// go itself failed before running anything (a toolchain it cannot get, a
	// broken go.mod): that is a run that could not happen, not an empty one.
	if len(result.Tests) == 0 && len(result.Packages) == 0 && len(s.failures) > 0 {
		return nil, fmt.Errorf("go test could not run:\n%s", strings.Join(s.failures, "\n"))
	}
	return result, nil
}

// session is the state of one Run.
type session struct {
	log      logging.Logger
	opts     Options
	tmp      string
	race     bool
	catalog  *catalog
	cover    *coverage
	profiles int // coverage profiles read
	stopped  bool
	failures []string // go test runs that failed as a whole, with what go said
	result   *Result

	progress    *progress
	invocations int // go test invocations started
	// Package ends expected over the run, and the package ends, top-level
	// tests and failed top-level tests of the invocations that finished.
	total, done, tests, failed int
}

// unitPlan is a unit with its environment and the invocations that run it.
type unitPlan struct {
	unit        loader.Unit
	rel         string
	env         []string
	invocations []invocation
}

func (s *session) diag(msg string) { s.result.Diagnostics = append(s.result.Diagnostics, msg) }

func (s *session) runInvocation(ctx context.Context, p unitPlan, inv invocation) {
	profile := ""
	if s.opts.Cover {
		profile = filepath.Join(s.tmp, fmt.Sprintf("cover-%d.out", s.invocations))
	}
	s.invocations++
	start := time.Now()
	run := s.goTest(ctx, p.unit, p.env, profile, inv)
	s.log.Info("tests run", "dir", p.unit.Dir, "run", inv.run, "elapsed", time.Since(start).Round(time.Millisecond).String(),
		"events", run.events.events, "exit", run.exitCode)
	if ctx.Err() != nil {
		s.stopped = true
	} else {
		s.runDiagnostics(run, p.rel)
	}
	s.collect(run)
	s.done += len(run.events.ended)
	s.tests += run.events.topTests
	s.failed += run.events.topFailed
	if profile != "" {
		s.readProfile(profile, p.rel)
	}
}

// report writes the progress of the run, with what c, the collector of the
// invocation going on, saw so far.
func (s *session) report(c *collector, force bool) {
	done, tests, failed, current := s.done, s.tests, s.failed, ""
	if c != nil {
		done += len(c.ended)
		tests += c.topTests
		failed += c.topFailed
		if c.current != "" {
			current = s.catalog.container(c.current)
		}
	}
	s.progress.update(done, s.total, tests, failed, current, force)
}

// collector returns a collector reporting progress as events arrive.
func (s *session) collector() *collector {
	c := newCollector()
	if s.progress != nil {
		c.notify = func(ended bool) { s.report(c, ended) }
	}
	return c
}

// goTest runs go test with the race detector when the session still has it,
// and again without it when the toolchain refuses it.
func (s *session) goTest(ctx context.Context, unit loader.Unit, env []string, profile string, inv invocation) unitRun {
	run := goTest(ctx, unit, env, profile, s.race, inv, s.collector())
	if !s.race {
		return run
	}
	why, ok := raceUnavailable(run)
	if !ok {
		return run
	}
	s.log.Warn("race detector unavailable", "reason", why)
	s.race = false
	s.result.Race = false
	s.diag("the race detector was off: the toolchain cannot use it here (" + why + ")")
	return goTest(ctx, unit, env, profile, false, inv, s.collector())
}

// runDiagnostics reports a go test that failed as a whole.
func (s *session) runDiagnostics(run unitRun, rel string) {
	if run.err != nil {
		s.diag(fmt.Sprintf("go test could not run in %s: %v", rel, run.err))
	}
	if run.events.events == 0 && run.exitCode != 0 {
		failure := fmt.Sprintf("go test failed in %s: %s", rel, strings.TrimSpace(keepEnd(run.stderr, 4<<10)))
		s.failures = append(s.failures, failure)
		s.diag(failure)
	}
}

func (s *session) readProfile(profile, rel string) {
	if data, err := os.Open(profile); err == nil {
		if err := s.cover.add(data); err != nil {
			s.diag(fmt.Sprintf("the coverage profile of %s could not be read: %v", rel, err))
		} else {
			s.profiles++
		}
		data.Close()
	}
}

// collect converts what the event stream said into the protocol's lists.
func (s *session) collect(run unitRun) {
	c := run.events
	for _, t := range c.tests {
		test := protocol.Test{
			Container: s.catalog.container(t.pkg), Package: s.catalog.container(t.pkg), Name: t.name, Hierarchy: hierarchy(t.name),
			Outcome: t.outcome, DurationMs: t.elapsed.Milliseconds(), Command: s.catalog.testCommand(run.dir, t.pkg, t.name, run.race),
		}
		if f, ok := s.catalog.testFunc(t.pkg, t.name); ok {
			id, file, line := f.ID, f.File, f.StartLine
			test.Container, test.Function, test.File, test.Line = f.Container, &id, &file, &line
		}
		if t.outcome == protocol.OutcomeFail {
			out := t.output
			test.Output = &out
		}
		s.result.Tests = append(s.result.Tests, test)
	}
	sections := stderrSections(run.stderr)
	for _, path := range c.ended {
		p := s.packageOutcome(c.results[path], c, sections, run.stderr)
		p.Command = s.catalog.packageCommand(run.dir, path, run.race)
		s.result.Packages = append(s.result.Packages, p)
	}
	for _, race := range c.races {
		r := protocol.Race{Container: s.catalog.container(race.pkg), Output: race.output}
		if race.test != "" {
			name := race.test
			r.Test = &name
			if f, ok := s.catalog.testFunc(race.pkg, race.test); ok {
				r.Container = f.Container
			}
		}
		s.result.Races = append(s.result.Races, r)
	}
}

// packageOutcome classifies the end of a package. A build failure is told
// by the "FailedBuild" field of recent toolchains, or by the "[build
// failed]" / "[setup failed]" line older ones print; its output is the build
// output of the failed build, from build events or else from the go
// command's stderr.
func (s *session) packageOutcome(r packageResult, c *collector, sections map[string]string, stderr string) protocol.TestPackage {
	p := protocol.TestPackage{Container: s.catalog.container(r.pkg), DurationMs: r.elapsed.Milliseconds()}
	setOutput := func(out string) {
		out = keepEnd(out, maxOutput)
		p.Output = &out
	}
	switch {
	case buildFailed(r):
		p.Outcome = protocol.OutcomeBuildFailed
		setOutput(buildOutput(r, c, sections, stderr))
	case r.action == "":
		p.Outcome = protocol.OutcomeFail
		setOutput("the package did not finish before the run was stopped\n" + r.output)
	case r.action == "fail":
		p.Outcome = protocol.OutcomeFail
		if !r.failedTests {
			setOutput(r.output)
		}
	case r.action == "pass" && r.ranTests:
		p.Outcome = protocol.OutcomePass
	default: // pass without tests, or skip ("no test files")
		p.Outcome = protocol.OutcomeNoTests
	}
	return p
}

func buildFailed(r packageResult) bool {
	return r.failedBuild != "" ||
		(r.action == "fail" && (strings.Contains(r.output, "[build failed]") || strings.Contains(r.output, "[setup failed]")))
}

// buildOutput finds the output of a failed build: its build events, else
// its section of stderr (the package's own, or the failed build's), else
// everything.
func buildOutput(r packageResult, c *collector, sections map[string]string, stderr string) string {
	if t := c.build[r.failedBuild]; r.failedBuild != "" && t != nil {
		return t.String()
	}
	if out := sections[r.pkg]; out != "" {
		return out
	}
	if out := sections[importPathOf(r.failedBuild)]; r.failedBuild != "" && out != "" {
		return out
	}
	return r.output + stderr
}

func (s *session) finish() *Result {
	res := s.result
	res.Packages = mergePackages(res.Packages)
	switch {
	case s.stopped:
		s.diag(fmt.Sprintf("the run was stopped after %s: results are partial and coverage was not measured", s.opts.Timeout))
	case !s.opts.Cover:
	case s.profiles == 0:
		s.diag("coverage was not measured: go test wrote no coverage profile")
	default:
		res.Coverage = &protocol.Coverage{Blocks: s.cover.blocks(s.catalog.sourceFile)}
	}
	if res.Tests == nil {
		res.Tests = []protocol.Test{}
	}
	if res.Packages == nil {
		res.Packages = []protocol.TestPackage{}
	}
	if res.Races == nil {
		res.Races = []protocol.Race{}
	}
	if res.Diagnostics == nil {
		res.Diagnostics = []string{}
	}
	return res
}

// unitRun is one go test invocation.
type unitRun struct {
	dir      string // where go test ran: the unit's directory
	race     bool   // with the race detector
	events   *collector
	stderr   string
	exitCode int
	err      error // could not start or wait, other than a non-zero exit
}

// goTest runs `go test -json` for an invocation of the unit, streaming its
// events to events.
//
// -vet=off: go test would otherwise run a subset of go vet first and report
// a vet finding as a build failure, hiding the package's test results; vet
// is the check command's job. -count=1 bypasses the test cache. With a
// profile, statement coverage covers every package of the unit (-coverpkg),
// counted (atomic with the race detector, which requires it).
func goTest(ctx context.Context, unit loader.Unit, env []string, profile string, race bool, inv invocation, events *collector) unitRun {
	args := []string{"test", "-json", "-count=1", "-vet=off"}
	mode := "count"
	if race {
		args = append(args, "-race")
		mode = "atomic"
	}
	if profile != "" {
		args = append(args,
			"-covermode="+mode,
			"-coverpkg="+strings.Join(unit.Patterns, ","),
			"-coverprofile="+profile,
		)
		_ = os.Remove(profile)
	}
	if inv.run != "" {
		args = append(args, "-run="+inv.run)
	}
	args = append(args, "-timeout="+binaryTimeout(ctx).String())
	args = append(args, inv.patterns...)

	run := unitRun{dir: unit.Dir, race: race, events: events}
	stderr := newTail(maxStderr)
	cmd := gocmd.Command(ctx, unit.Dir, env, "go", args...)
	cmd.Stderr = stderr
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		run.err = err
		return run
	}
	if err := cmd.Start(); err != nil {
		run.err = err
		return run
	}
	readErr := run.events.read(stdout)
	waitErr := cmd.Wait()
	run.events.finish(time.Now())
	run.stderr = stderr.String()
	run.exitCode = cmd.ProcessState.ExitCode()
	var exit *exec.ExitError
	switch {
	case readErr != nil:
		run.err = readErr
	case waitErr != nil && !errors.As(waitErr, &exit) && ctx.Err() == nil:
		run.err = waitErr
	}
	return run
}

// binaryTimeout is the -timeout given to each test binary: the time left,
// minus a margin so that go test reports a binary's timeout (with the stuck
// tests) before the whole run is stopped.
func binaryTimeout(ctx context.Context) time.Duration {
	deadline, ok := ctx.Deadline()
	if !ok {
		return 0 // no timeout
	}
	left := time.Until(deadline)
	margin := min(left/10, 5*time.Second)
	return max(left-margin, time.Second).Round(time.Millisecond)
}

// raceUnavailable reports whether go test refused -race before running
// anything, with the go command's reason: no cgo where the race detector
// needs it, or a platform without it.
func raceUnavailable(run unitRun) (string, bool) {
	if run.events.events > 0 {
		return "", false
	}
	for line := range strings.SplitSeq(run.stderr, "\n") {
		if strings.Contains(line, "-race requires cgo") || strings.Contains(line, "-race is not supported") {
			return strings.TrimSpace(line), true
		}
	}
	return "", false
}

// testEnv is the environment of go test for the unit. Modules use -mod=mod
// so that an untidy go.mod is completed from the module cache (the tree is a
// throwaway copy); workspaces, where -mod=mod is not allowed, use readonly,
// and vendored units their vendor directory. Offline, nothing may be
// downloaded: no proxy, no direct fetch of private modules. The checksum
// database stays on: go answers from what prepare verified and cached, and
// turning it off makes go refuse the toolchain it switches to when go.mod
// asks for a newer Go than the installed one ("verifying module: checksum
// database disabled by GOSUMDB=off").
func testEnv(base []string, unit loader.Unit, offline bool) []string {
	mod := "mod"
	switch {
	case unit.Vendor:
		mod = "vendor"
	case unit.GoWork != "off":
		mod = "readonly"
	}
	env := unit.Env(base, mod)
	if offline {
		env = gocmd.SetEnv(env, "GOPROXY", "off")
		env = gocmd.SetEnv(env, "GONOPROXY", "none")
	}
	return env
}

// BaseEnv is the environment a run of the tests starts from, as Run builds
// it (see baseEnv), for the commands that run tests their own way (mutate).
func (r *Runner) BaseEnv(ctx context.Context, unit loader.Unit, cacheDir string) []string {
	return r.baseEnv(ctx, unit, cacheDir)
}

// TestEnv is the environment go test runs with for the unit, as Run builds
// it from base (see testEnv).
func TestEnv(base []string, unit loader.Unit, offline bool) []string {
	return testEnv(base, unit, offline)
}

// baseEnv is the environment the run starts from: the analyzer's own, with
// GOCACHE in cacheDir when the user's build cache is not usable there (see
// gocmd.BuildCache), as prepare reported it. Without a cacheDir it is left
// to the go command.
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

// stderrSections splits the go command's stderr into the sections it heads
// with "# <import path>" lines, as older toolchains print build errors.
func stderrSections(stderr string) map[string]string {
	sections := map[string]string{}
	current := ""
	for line := range strings.SplitAfterSeq(stderr, "\n") {
		if header, ok := strings.CutPrefix(line, "# "); ok {
			current = importPathOf(strings.TrimSpace(header))
		}
		if current != "" {
			sections[current] += line
		}
	}
	return sections
}

// importPathOf returns the import path of a package as the go command names
// builds: "example.com/p [example.com/p.test]" is example.com/p.
func importPathOf(build string) string {
	path, _, _ := strings.Cut(build, " ")
	return path
}

func relDir(tree analyzer.Tree, dir string) string {
	if rel, ok := tree.Rel(dir); ok {
		return rel
	}
	return "."
}
