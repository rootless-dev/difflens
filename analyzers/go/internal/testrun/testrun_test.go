package testrun

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// stream encodes events as `go test -json` prints them.
func stream(t *testing.T, events ...event) string {
	t.Helper()
	var b strings.Builder
	for _, ev := range events {
		data, err := json.Marshal(ev)
		if err != nil {
			t.Fatal(err)
		}
		b.Write(data)
		b.WriteByte('\n')
	}
	return b.String()
}

func collect(t *testing.T, text string) *collector {
	t.Helper()
	c := newCollector()
	if err := c.read(strings.NewReader(text)); err != nil {
		t.Fatal(err)
	}
	c.finish(time.Now())
	return c
}

func newSession() *session {
	return &session{log: logging.Discard(), catalog: newCatalog(analyzer.Tree{}), cover: newCoverage(), result: &Result{}}
}

func outputOf(t *testing.T, s *string) string {
	t.Helper()
	if s == nil {
		t.Fatal("output is null")
	}
	return *s
}

func TestSubtestOutputGoesToItsParent(t *testing.T) {
	const pkg = "example.com/p"
	c := collect(t, stream(t,
		event{Action: "run", Package: pkg, Test: "TestA"},
		event{Action: "output", Package: pkg, Test: "TestA", Output: "=== RUN   TestA\n"},
		event{Action: "run", Package: pkg, Test: "TestA/one"},
		event{Action: "output", Package: pkg, Test: "TestA/one", Output: "    a_test.go:9: boom\n"},
		event{Action: "fail", Package: pkg, Test: "TestA/one", Elapsed: 0.25},
		event{Action: "run", Package: pkg, Test: "TestA/two"},
		event{Action: "pass", Package: pkg, Test: "TestA/two"},
		event{Action: "fail", Package: pkg, Test: "TestA", Elapsed: 0.3},
		event{Action: "fail", Package: pkg, Elapsed: 1},
	))
	names := []string{}
	for _, r := range c.tests {
		names = append(names, r.name+":"+r.outcome)
	}
	if got := strings.Join(names, " "); got != "TestA/one:fail TestA/two:pass TestA:fail" {
		t.Errorf("tests in finish order: %s", got)
	}
	if !strings.Contains(c.tests[2].output, "a_test.go:9: boom") || !strings.Contains(c.tests[2].output, "=== RUN   TestA\n") {
		t.Errorf("parent output misses the subtest's: %q", c.tests[2].output)
	}
	if c.tests[1].output != "" {
		t.Errorf("a passing test has output: %q", c.tests[1].output)
	}
	if c.tests[0].elapsed != 250*time.Millisecond {
		t.Errorf("elapsed: %v", c.tests[0].elapsed)
	}
	s := newSession()
	s.collect(unitRun{events: c})
	if p := s.result.Packages[0]; p.Outcome != protocol.OutcomeFail || p.Output != nil || p.DurationMs != 1000 {
		t.Errorf("package explained by its failed test: %+v", p)
	}
}

func TestOutputKeepsTheEnd(t *testing.T) {
	const pkg = "example.com/p"
	events := []event{{Action: "run", Package: pkg, Test: "TestBig"}}
	line := strings.Repeat("é", 50) + "\n"
	for range 2000 {
		events = append(events, event{Action: "output", Package: pkg, Test: "TestBig", Output: line})
	}
	events = append(events,
		event{Action: "output", Package: pkg, Test: "TestBig", Output: "--- FAIL: TestBig\n"},
		event{Action: "fail", Package: pkg, Test: "TestBig"},
		event{Action: "fail", Package: pkg})
	c := collect(t, stream(t, events...))
	out := c.tests[0].output
	if len(out) > maxOutput || len(out) < maxOutput-4 {
		t.Errorf("output is %d bytes, want at most %d", len(out), maxOutput)
	}
	if !strings.HasSuffix(out, "--- FAIL: TestBig\n") {
		t.Error("the end of the output was not kept")
	}
	if !strings.HasPrefix(out, "é") && !strings.HasPrefix(out, "\n") {
		t.Errorf("output starts inside a rune: %q", out[:4])
	}
}

func TestTestsStillRunningWhenThePackageEndsFailed(t *testing.T) {
	const pkg = "example.com/p"
	start := time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
	c := collect(t, stream(t,
		event{Time: start, Action: "run", Package: pkg, Test: "TestHang"},
		event{Time: start, Action: "run", Package: pkg, Test: "TestHang/forever"},
		event{Time: start, Action: "output", Package: pkg, Test: "TestHang/forever", Output: "panic: test timed out after 1s\n"},
		event{Time: start.Add(2 * time.Second), Action: "fail", Package: pkg, Elapsed: 2},
	))
	if len(c.tests) != 2 || c.tests[0].name != "TestHang/forever" || c.tests[1].name != "TestHang" {
		t.Fatalf("tests: %+v", c.tests)
	}
	for _, r := range c.tests {
		if r.outcome != "fail" || !strings.Contains(r.output, "timed out") || r.elapsed != 2*time.Second {
			t.Errorf("%+v", r)
		}
	}
}

func TestBuildFailures(t *testing.T) {
	t.Run("build events of Go 1.24 and later", func(t *testing.T) {
		c := collect(t, stream(t,
			event{ImportPath: "example.com/p [example.com/p.test]", Action: "build-output", Output: "# example.com/p [example.com/p.test]\n"},
			event{ImportPath: "example.com/p [example.com/p.test]", Action: "build-output", Output: "p/p_test.go:6:16: undefined: missing\n"},
			event{ImportPath: "example.com/p [example.com/p.test]", Action: "build-fail"},
			event{Action: "start", Package: "example.com/p"},
			event{Action: "output", Package: "example.com/p", Output: "FAIL\texample.com/p [build failed]\n"},
			event{Action: "fail", Package: "example.com/p", FailedBuild: "example.com/p [example.com/p.test]"},
		))
		s := newSession()
		s.collect(unitRun{events: c})
		p := s.result.Packages[0]
		if p.Outcome != protocol.OutcomeBuildFailed || !strings.Contains(outputOf(t, p.Output), "undefined: missing") {
			t.Errorf("%+v", p)
		}
	})
	t.Run("stderr of older toolchains", func(t *testing.T) {
		c := collect(t, stream(t,
			event{Action: "output", Package: "example.com/p", Output: "FAIL\texample.com/p [build failed]\n"},
			event{Action: "fail", Package: "example.com/p"},
			event{Action: "output", Package: "example.com/q", Output: "FAIL\texample.com/q [setup failed]\n"},
			event{Action: "fail", Package: "example.com/q"},
		))
		s := newSession()
		s.collect(unitRun{events: c, stderr: "# example.com/p [example.com/p.test]\np/p_test.go:6:16: undefined: missing\n# example.com/q\nq/q.go:3:8: no required module provides package x\n"})
		for i, want := range []string{"undefined: missing", "no required module"} {
			p := s.result.Packages[i]
			out := outputOf(t, p.Output)
			if p.Outcome != protocol.OutcomeBuildFailed || !strings.Contains(out, want) || strings.Contains(out, "\n# ") {
				t.Errorf("%+v %q", p, out)
			}
		}
	})
}

func TestPackageOutcomes(t *testing.T) {
	c := collect(t, stream(t,
		event{Action: "output", Package: "example.com/none", Output: "?   \texample.com/none\t[no test files]\n"},
		event{Action: "skip", Package: "example.com/none"},
		event{Action: "output", Package: "example.com/empty", Output: "ok  \texample.com/empty\t0.01s [no tests to run]\n"},
		event{Action: "pass", Package: "example.com/empty"},
		event{Action: "run", Package: "example.com/skips", Test: "TestS"},
		event{Action: "skip", Package: "example.com/skips", Test: "TestS"},
		event{Action: "pass", Package: "example.com/skips"},
		event{Action: "output", Package: "example.com/main", Output: "panic: setup\n"},
		event{Action: "fail", Package: "example.com/main"},
		event{Action: "run", Package: "example.com/killed", Test: "TestK"},
	))
	s := newSession()
	s.collect(unitRun{events: c})
	got := map[string]protocol.TestPackage{}
	for _, p := range s.result.Packages {
		got[p.Container] = p
	}
	for container, want := range map[string]string{
		"example.com/none":   protocol.OutcomeNoTests,
		"example.com/empty":  protocol.OutcomeNoTests,
		"example.com/skips":  protocol.OutcomePass,
		"example.com/main":   protocol.OutcomeFail,
		"example.com/killed": protocol.OutcomeFail,
	} {
		if got[container].Outcome != want {
			t.Errorf("%s: got %q, want %q", container, got[container].Outcome, want)
		}
	}
	if out := outputOf(t, got["example.com/main"].Output); !strings.Contains(out, "panic: setup") {
		t.Errorf("failure outside tests has no output: %q", out)
	}
	if out := outputOf(t, got["example.com/killed"].Output); !strings.Contains(out, "did not finish") {
		t.Errorf("stopped package: %q", out)
	}
}

func TestRaceReports(t *testing.T) {
	const pkg = "example.com/p"
	report := []string{"==================\n", "WARNING: DATA RACE\n", "Read at 0x1 by goroutine 7:\n", "  p.f()\n", "==================\n"}
	var events []event
	events = append(events, event{Action: "run", Package: pkg, Test: "TestR/sub"})
	for _, line := range report {
		events = append(events, event{Action: "output", Package: pkg, Test: "TestR/sub", Output: line})
	}
	events = append(events, event{Action: "fail", Package: pkg, Test: "TestR/sub"})
	for _, line := range report {
		events = append(events, event{Action: "output", Package: pkg, Output: line})
	}
	events = append(events, event{Action: "fail", Package: pkg})
	c := collect(t, stream(t, events...))
	if len(c.races) != 2 {
		t.Fatalf("races: %+v", c.races)
	}
	if c.races[0].test != "TestR/sub" || c.races[1].test != "" {
		t.Errorf("attribution: %+v", c.races)
	}
	if want := "WARNING: DATA RACE\nRead at 0x1 by goroutine 7:\n  p.f()\n"; c.races[0].output != want {
		t.Errorf("report: %q", c.races[0].output)
	}
	s := newSession()
	s.collect(unitRun{events: c})
	if s.result.Races[0].Test == nil || *s.result.Races[0].Test != "TestR/sub" || s.result.Races[1].Test != nil {
		t.Errorf("%+v", s.result.Races)
	}
}

func TestRaceReportIsCapped(t *testing.T) {
	var s raceScanner
	s.feed("WARNING: DATA RACE\n")
	for range 1000 {
		s.feed(strings.Repeat("ü", 30) + "\n")
	}
	report, ok := s.feed("==================\n")
	if !ok || len(report) > maxRaceReport || !strings.HasPrefix(report, "WARNING: DATA RACE") {
		t.Errorf("report of %d bytes, ok=%v", len(report), ok)
	}
	if !utf8.ValidString(report) {
		t.Error("the cut split a rune")
	}
	if _, ok := s.flush(); ok {
		t.Error("a closed report flushed again")
	}
}

func TestCoverageMerge(t *testing.T) {
	c := newCoverage()
	profiles := []string{
		"mode: atomic\nexample.com/p/a.go:3.10,5.2 2 1\nexample.com/p/a.go:7.1,7.9 1 0\nexample.com/p/a_test.go:1.1,2.2 1 4\n",
		"mode: atomic\nexample.com/p/a.go:3.10,5.2 2 3\nexample.com/p/a.go:7.1,7.9 1 0\nexample.com/p/gen.go:1.1,1.9 1 1\n",
	}
	for _, p := range profiles {
		if err := c.add(strings.NewReader(p)); err != nil {
			t.Fatal(err)
		}
	}
	source := func(name string) (string, bool) {
		if strings.HasSuffix(name, "_test.go") || strings.HasSuffix(name, "gen.go") {
			return "", false
		}
		return strings.TrimPrefix(name, "example.com/"), true
	}
	blocks := c.blocks(source)
	want := []protocol.CoverageBlock{
		{File: "p/a.go", StartLine: 3, EndLine: 5, Statements: 2, Count: 4},
		{File: "p/a.go", StartLine: 7, EndLine: 7, Statements: 1, Count: 0},
	}
	if len(blocks) != len(want) || blocks[0] != want[0] || blocks[1] != want[1] {
		t.Errorf("got %+v, want %+v", blocks, want)
	}

	set := newCoverage()
	for range 2 {
		if err := set.add(strings.NewReader("mode: set\nexample.com/p/a.go:3.10,5.2 2 1\n")); err != nil {
			t.Fatal(err)
		}
	}
	if b := set.blocks(source); b[0].Count != 1 {
		t.Errorf("set mode merged to %d, want 1", b[0].Count)
	}
	if err := newCoverage().add(strings.NewReader("mode: set\ngarbage\n")); err == nil {
		t.Error("a malformed profile was accepted")
	}
}

// A go.mod asking for a Go that does not exist, with toolchain switching off:
// go fails before running any package, and the run fails instead of reporting
// zero tests.
func TestRunFailsWhenGoCannotRunAtAll(t *testing.T) {
	dir := t.TempDir()
	write := func(name, content string) {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("go.mod", "module example.com/future\n\ngo 1.999\n")
	write("future.go", "package future\n")
	write("future_test.go", "package future\n\nimport \"testing\"\n\nfunc TestNothing(t *testing.T) {}\n")
	t.Setenv("GOTOOLCHAIN", "local")

	result, err := New(logging.Discard()).Run(context.Background(), dir, Options{Timeout: time.Minute})

	if err == nil {
		t.Fatalf("the run succeeded with %d tests and %d packages", len(result.Tests), len(result.Packages))
	}
	if !strings.Contains(err.Error(), "go test could not run") || !strings.Contains(err.Error(), "1.999") {
		t.Errorf("the error does not say what go said: %v", err)
	}
}

func TestTestEnvironment(t *testing.T) {
	env := testEnv([]string{"GOPROXY=https://proxy.example", "GOFLAGS=-mod=readonly -tags=x"}, unitFor("off", false), true)
	joined := strings.Join(env, "\n")
	for _, want := range []string{"GOPROXY=off", "GONOPROXY=none", "GOFLAGS=-mod=mod -buildvcs=false -tags=x", "GOWORK=off"} {
		if !strings.Contains(joined, want) {
			t.Errorf("missing %s in %q", want, env)
		}
	}
	// Off, go refuses the toolchain it switches to for a go.mod newer than the installed Go.
	if strings.Contains(joined, "GOSUMDB") {
		t.Errorf("the checksum database is turned off: %q", env)
	}
	if strings.Contains(joined, "proxy.example") {
		t.Error("the user's proxy survived --offline")
	}
	if env := strings.Join(testEnv(nil, unitFor("/w/go.work", false), false), "\n"); !strings.Contains(env, "-mod=readonly") || strings.Contains(env, "GOPROXY") {
		t.Errorf("workspace online: %q", env)
	}
	if env := strings.Join(testEnv(nil, unitFor("off", true), true), "\n"); !strings.Contains(env, "-mod=vendor") {
		t.Errorf("vendored: %q", env)
	}
}

func TestRaceUnavailable(t *testing.T) {
	for stderr, want := range map[string]bool{
		"go: -race requires cgo; enable cgo by setting CGO_ENABLED=1\n": true,
		"-race is not supported on linux/mips\n":                        true,
		"go: no packages to test\n":                                     false,
	} {
		why, ok := raceUnavailable(unitRun{events: newCollector(), stderr: stderr, exitCode: 2})
		if ok != want || (ok && !strings.Contains(why, "-race")) {
			t.Errorf("%q: got %q %v", stderr, why, ok)
		}
	}
	// Once tests ran, the detector was available whatever stderr says.
	c := collect(t, stream(t, event{Action: "pass", Package: "example.com/p"}))
	if _, ok := raceUnavailable(unitRun{events: c, stderr: "-race requires cgo"}); ok {
		t.Error("a run with events was taken for a refused -race")
	}
}

func unitFor(goWork string, vendor bool) loader.Unit {
	return loader.Unit{Dir: "/src", GoWork: goWork, Vendor: vendor, Patterns: []string{"./..."}}
}

func TestRunPatterns(t *testing.T) {
	sub := func(name string, subtests ...string) protocol.SelectedTest {
		if subtests == nil {
			subtests = []string{}
		}
		return protocol.SelectedTest{Name: name, Subtests: subtests}
	}
	whole := func(name string) protocol.SelectedTest { return protocol.SelectedTest{Name: name} }
	for _, c := range []struct {
		tests []protocol.SelectedTest
		want  []string
	}{
		{[]protocol.SelectedTest{whole("TestA")}, []string{"^TestA$"}},
		{[]protocol.SelectedTest{whole("TestA"), whole("TestB"), whole("TestA")}, []string{"^(TestA|TestB)$"}},
		// Each test with only some subtests runs on its own.
		{
			[]protocol.SelectedTest{whole("TestA"), sub("TestB", "x", "y"), sub("TestC", "z")},
			[]string{"^TestA$", "^TestB$/^(x|y)$", "^TestC$/^z$"},
		},
		// Whole wins; nested subtests run their first-level ancestor; nothing selected runs nothing.
		{[]protocol.SelectedTest{sub("TestA", "x"), whole("TestA")}, []string{"^TestA$"}},
		{[]protocol.SelectedTest{sub("TestA", "x/deep", "x/other", "y")}, []string{"^TestA$/^(x|y)$"}},
		{[]protocol.SelectedTest{sub("TestA")}, nil},
		// Regular expression characters are quoted.
		{[]protocol.SelectedTest{sub("TestA", "v1,_the_(edge)", "a+b")}, []string{`^TestA$/^(v1,_the_\(edge\)|a\+b)$`}},
	} {
		if got := runPatterns(c.tests); !slices.Equal(got, c.want) {
			t.Errorf("%+v: got %q, want %q", c.tests, got, c.want)
		}
	}
}

func TestPlan(t *testing.T) {
	containers := map[string]string{"example.com/p/a": "a", "example.com/p/b": "b", "example.com/p/c": "c"}
	container := func(path string) string { return containers[path] }
	listed := []string{"example.com/p/a", "example.com/p/b", "example.com/p/c"}

	invs, total := plan([]string{"./..."}, listed, nil, container, map[string]bool{})
	if len(invs) != 1 || !slices.Equal(invs[0].patterns, []string{"./..."}) || invs[0].run != "" || total != 3 {
		t.Errorf("whole run: %+v, total %d", invs, total)
	}

	found := map[string]bool{}
	sel := &protocol.Selection{Packages: []protocol.SelectedPackage{
		{Package: "c"},
		{Package: "b", Tests: []protocol.SelectedTest{{Name: "TestX"}, {Name: "TestY", Subtests: []string{"one"}}}},
		{Package: "a"},
		{Package: "elsewhere"},
	}}
	invs, total = plan([]string{"./..."}, listed, sel, container, found)
	got := []string{}
	for _, inv := range invs {
		got = append(got, strings.Join(inv.patterns, ",")+" "+inv.run)
	}
	want := []string{"example.com/p/a,example.com/p/c ", "example.com/p/b ^TestX$", "example.com/p/b ^TestY$/^one$"}
	if !slices.Equal(got, want) || total != 4 {
		t.Errorf("selection: %q total %d, want %q total 4", got, total, want)
	}
	if !found["a"] || !found["b"] || !found["c"] || found["elsewhere"] {
		t.Errorf("found: %v", found)
	}
}

func TestMergePackages(t *testing.T) {
	text := func(s string) *string { return &s }
	merged := mergePackages([]protocol.TestPackage{
		{Container: "a", Outcome: protocol.OutcomePass, DurationMs: 10},
		{Container: "b", Outcome: protocol.OutcomeNoTests, DurationMs: 1},
		{Container: "a", Outcome: protocol.OutcomeFail, DurationMs: 5, Output: text("panic: setup\n")},
		{Container: "b", Outcome: protocol.OutcomePass, DurationMs: 2},
		{Container: "a", Outcome: protocol.OutcomePass, DurationMs: 1},
	})
	if len(merged) != 2 {
		t.Fatalf("%+v", merged)
	}
	if a := merged[0]; a.Outcome != protocol.OutcomeFail || a.DurationMs != 16 || str(a.Output) != "panic: setup\n" {
		t.Errorf("a: %+v", a)
	}
	if b := merged[1]; b.Outcome != protocol.OutcomePass || b.DurationMs != 3 || b.Output != nil {
		t.Errorf("b: %+v", b)
	}
}

func str(s *string) string {
	if s == nil {
		return "<nil>"
	}
	return *s
}

func TestProgressIsWrittenWholeAndThrottled(t *testing.T) {
	if newProgress("") != nil {
		t.Error("no path, no progress")
	}
	var none *progress
	none.update(1, 2, 3, 4, "x", true) // a nil progress writes nothing

	path := filepath.Join(t.TempDir(), "progress.json")
	p := newProgress(path)
	p.update(0, 5, 0, 0, "", true)
	p.update(1, 5, 7, 1, "app/x", false) // too soon: not written
	read := func() protocol.Progress {
		t.Helper()
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var doc protocol.Progress
		if err := json.Unmarshal(data, &doc); err != nil {
			t.Fatal(err)
		}
		return doc
	}
	if doc := read(); doc.PackagesDone != 0 || doc.PackagesTotal != 5 || doc.Current != nil || doc.Protocol != protocol.Version {
		t.Errorf("first: %+v", doc)
	}
	p.update(2, 1, 9, 1, "app/y", true)
	doc := read()
	if doc.PackagesDone != 2 || doc.PackagesTotal != 2 || doc.Tests != 9 || doc.Failed != 1 || doc.Current == nil || *doc.Current != "app/y" {
		t.Errorf("forced: %+v (the total is never below what is done)", doc)
	}
	entries, _ := os.ReadDir(filepath.Dir(path))
	if len(entries) != 1 {
		t.Errorf("temporary files left: %v", entries)
	}
}

func TestCollectorCountsProgress(t *testing.T) {
	const pkg = "example.com/p"
	c := newCollector()
	var ends, calls int
	c.notify = func(ended bool) {
		calls++
		if ended {
			ends++
		}
	}
	if err := c.read(strings.NewReader(stream(t,
		event{Action: "run", Package: pkg, Test: "TestA"},
		event{Action: "run", Package: pkg, Test: "TestA/x"},
		event{Action: "fail", Package: pkg, Test: "TestA/x"},
		event{Action: "fail", Package: pkg, Test: "TestA"},
		event{Action: "run", Package: pkg, Test: "TestB"},
		event{Action: "pass", Package: pkg, Test: "TestB"},
		event{Action: "fail", Package: pkg},
	))); err != nil {
		t.Fatal(err)
	}
	if c.topTests != 2 || c.topFailed != 1 || c.current != "" || ends != 1 || calls != 7 {
		t.Errorf("tests %d failed %d current %q ends %d calls %d", c.topTests, c.topFailed, c.current, ends, calls)
	}
}

func TestBinaryTimeoutLeavesAMargin(t *testing.T) {
	if d := binaryTimeout(t.Context()); d != 0 {
		t.Errorf("no deadline: %v", d)
	}
	ctx, cancel := context.WithTimeout(t.Context(), time.Minute)
	defer cancel()
	if d := binaryTimeout(ctx); d < 54*time.Second || d > 55*time.Second {
		t.Errorf("one minute left: %v, want about 55s", d)
	}
}
