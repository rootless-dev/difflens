package app

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/santhosh-tekuri/jsonschema/v6"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/checks"
	"github.com/rootless-dev/difflens/analyzers/go/internal/entrypoints"
	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
	"github.com/rootless-dev/difflens/analyzers/go/internal/testrun"
)

const (
	testsSchemaPath  = "../../../protocol/tests.schema.json"
	checksSchemaPath = "../../../protocol/checks.schema.json"
)

func requireSchema(t *testing.T, ref string, out []byte) {
	t.Helper()
	schema, err := jsonschema.NewCompiler().Compile(ref)
	if err != nil {
		t.Fatal(err)
	}
	inst, err := jsonschema.UnmarshalJSON(bytes.NewReader(out))
	if err != nil {
		t.Fatalf("output is not JSON: %v\n%s", err, out)
	}
	if err := schema.Validate(inst); err != nil {
		t.Errorf("output does not match %s: %v", ref, err)
	}
}

// fixtureCopy copies testdata/<fixture> to a temporary directory, as the
// core gives the test command a throwaway copy of the tree.
func fixtureCopy(t *testing.T, fixture string) string {
	t.Helper()
	dir := filepath.Join(t.TempDir(), fixture)
	copyTree(t, filepath.Join("testdata", fixture), dir)
	return dir
}

func runTests(t *testing.T, dir string, extra ...string) (*protocol.TestRun, []byte) {
	t.Helper()
	args := append([]string{"test", "--dir", dir, "--timeout", "2m", "--offline"}, extra...)
	out, stderr, code := runCLI(t, args...)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	var doc protocol.TestRun
	if err := json.Unmarshal(out, &doc); err != nil {
		t.Fatalf("decode: %v", err)
	}
	return &doc, out
}

func testByName(t *testing.T, doc *protocol.TestRun, name string) protocol.Test {
	t.Helper()
	for _, test := range doc.Tests {
		if test.Name == name {
			return test
		}
	}
	t.Fatalf("test %s not reported; tests: %+v", name, doc.Tests)
	return protocol.Test{}
}

func packageByContainer(t *testing.T, doc *protocol.TestRun, container string) protocol.TestPackage {
	t.Helper()
	for _, p := range doc.Packages {
		if p.Container == container {
			return p
		}
	}
	t.Fatalf("package %s not reported; packages: %+v", container, doc.Packages)
	return protocol.TestPackage{}
}

func readFile(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func str(s *string) string {
	if s == nil {
		return "<nil>"
	}
	return *s
}

func TestTestCommand(t *testing.T) {
	dir := fixtureCopy(t, "testrun")
	doc, out := runTests(t, dir)
	requireSchema(t, testsSchemaPath, out)
	if !doc.Race || doc.Runner.Version != Version || doc.Language != "go" {
		t.Errorf("header: race=%v runner=%+v language=%s", doc.Race, doc.Runner, doc.Language)
	}

	t.Run("outcomes", func(t *testing.T) {
		for name, want := range map[string]string{
			"TestAdd":          protocol.OutcomePass,
			"TestDiv/by_one":   protocol.OutcomePass,
			"TestDiv/by_zero":  protocol.OutcomeFail,
			"TestDiv":          protocol.OutcomeFail,
			"TestSkipped":      protocol.OutcomeSkip,
			"TestCounterRace":  protocol.OutcomeFail,
			"TestExternal":     protocol.OutcomePass,
			"ExampleAdd":       protocol.OutcomePass,
			"TestGreeting":     protocol.OutcomePass,
			"TestOnlyExternal": protocol.OutcomePass,
		} {
			if got := testByName(t, doc, name); got.Outcome != want {
				t.Errorf("%s: got %s, want %s", name, got.Outcome, want)
			}
		}
		if len(doc.Tests) != 10 {
			t.Errorf("got %d tests, want 10", len(doc.Tests))
		}
		div := testByName(t, doc, "TestDiv")
		if out := str(div.Output); !strings.Contains(out, "dividing by zero") || !strings.Contains(out, "unexpected error: division by zero") {
			t.Errorf("a failed parent must carry its subtests' output: %q", out)
		}
		if add := testByName(t, doc, "TestAdd"); add.Output != nil {
			t.Errorf("a passing test has output: %q", *add.Output)
		}
		// Subtests finish before their parent.
		if slices.IndexFunc(doc.Tests, func(x protocol.Test) bool { return x.Name == "TestDiv/by_zero" }) >
			slices.IndexFunc(doc.Tests, func(x protocol.Test) bool { return x.Name == "TestDiv" }) {
			t.Error("tests are not in the order they finished")
		}
	})

	t.Run("functions", func(t *testing.T) {
		for name, want := range map[string]struct{ container, function, file string }{
			"TestDiv/by_zero": {"calc", "example.com/testrun/calc.TestDiv", "calc/calc_test.go"},
			"TestExternal":    {"calc", "example.com/testrun/calc_test.TestExternal", "calc/external_test.go"},
			"ExampleAdd":      {"calc", "example.com/testrun/calc_test.ExampleAdd", "calc/external_test.go"},
			"TestGreeting":    {"testrun_test", "example.com/testrun_test.TestGreeting", "root_test.go"},
			// A directory with only an external test package.
			"TestOnlyExternal": {"extonly", "example.com/testrun/extonly_test.TestOnlyExternal", "extonly/extonly_test.go"},
		} {
			got := testByName(t, doc, name)
			if got.Container != want.container || str(got.Function) != want.function || str(got.File) != want.file || got.Line == nil {
				t.Errorf("%s: got %s %s %s, want %+v", name, got.Container, str(got.Function), str(got.File), want)
			}
		}
		if line := testByName(t, doc, "TestDiv/by_zero").Line; line == nil || *line != 17 {
			t.Errorf("TestDiv line: %v", line)
		}
		// The package a test ran in is named as packages lists it, also when
		// the function is listed elsewhere.
		for name, want := range map[string]string{"TestGreeting": "testrun", "TestExternal": "calc", "TestOnlyExternal": "extonly"} {
			if got := testByName(t, doc, name).Package; got != want {
				t.Errorf("%s ran in package %q, want %q", name, got, want)
			}
		}
	})

	t.Run("hierarchy and commands", func(t *testing.T) {
		for name, want := range map[string]struct {
			hierarchy []string
			command   string
		}{
			"TestDiv/by_zero": {[]string{"TestDiv", "by_zero"}, `go test -race -run '^TestDiv$/^by_zero$' ./calc`},
			"TestDiv":         {[]string{"TestDiv"}, `go test -race -run '^TestDiv$' ./calc`},
			"TestExternal":    {[]string{"TestExternal"}, `go test -race -run '^TestExternal$' ./calc`},
			// The root package, whose container is its name.
			"TestGreeting":     {[]string{"TestGreeting"}, `go test -race -run '^TestGreeting$' .`},
			"TestOnlyExternal": {[]string{"TestOnlyExternal"}, `go test -race -run '^TestOnlyExternal$' ./extonly`},
		} {
			got := testByName(t, doc, name)
			if !slices.Equal(got.Hierarchy, want.hierarchy) || str(got.Command) != want.command {
				t.Errorf("%s: got %q %s, want %q %s", name, got.Hierarchy, str(got.Command), want.hierarchy, want.command)
			}
		}
		for container, want := range map[string]string{
			"calc": "go test -race ./calc", "testrun": "go test -race .", "broken": "go test -race ./broken",
		} {
			if got := packageByContainer(t, doc, container); str(got.Command) != want {
				t.Errorf("package %s: got %s, want %s", container, str(got.Command), want)
			}
		}
	})

	t.Run("packages", func(t *testing.T) {
		for container, want := range map[string]string{
			"calc":    protocol.OutcomeFail,
			"broken":  protocol.OutcomeBuildFailed,
			"setup":   protocol.OutcomeFail,
			"notests": protocol.OutcomeNoTests,
			"testrun": protocol.OutcomePass,
			"extonly": protocol.OutcomePass,
		} {
			if got := packageByContainer(t, doc, container); got.Outcome != want {
				t.Errorf("%s: got %s, want %s", container, got.Outcome, want)
			}
		}
		if out := str(packageByContainer(t, doc, "broken").Output); !strings.Contains(out, "broken/broken_test.go:6:16: undefined: missing") {
			t.Errorf("build failure output: %q", out)
		}
		if out := str(packageByContainer(t, doc, "setup").Output); !strings.Contains(out, "panic: setup is not ready") {
			t.Errorf("a failure outside tests must carry the package output: %q", out)
		}
		if calc := packageByContainer(t, doc, "calc"); calc.Output != nil {
			t.Errorf("a package explained by its failed tests has output: %q", *calc.Output)
		}
	})

	t.Run("races", func(t *testing.T) {
		if len(doc.Races) != 1 {
			t.Fatalf("races: %+v", doc.Races)
		}
		r := doc.Races[0]
		if r.Container != "calc" || str(r.Test) != "TestCounterRace" || !strings.HasPrefix(r.Output, "WARNING: DATA RACE") || !strings.Contains(r.Output, "(*Counter).Inc") {
			t.Errorf("race: %+v", r)
		}
	})

	t.Run("coverage", func(t *testing.T) {
		if doc.Coverage == nil {
			t.Fatalf("coverage is null; diagnostics: %q", doc.Diagnostics)
		}
		files := map[string]bool{}
		covered := map[string]bool{}
		for _, b := range doc.Coverage.Blocks {
			files[b.File] = true
			if strings.HasSuffix(b.File, "_test.go") || strings.HasSuffix(b.File, "zz_generated.go") {
				t.Errorf("test or generated file in coverage: %+v", b)
			}
			if b.Count > 0 {
				covered[b.File+":"+strconv.Itoa(b.StartLine)] = true
			}
		}
		for _, f := range []string{"calc/calc.go", "notests/notests.go", "root.go", "broken/broken.go"} {
			if !files[f] {
				t.Errorf("no coverage blocks for %s", f)
			}
		}
		if !covered["calc/calc.go:5"] || !covered["root.go:5"] {
			t.Errorf("Add or Greeting not covered: %v", covered)
		}
		if covered["calc/calc.go:16"] {
			t.Error("Never is reported as covered")
		}
		// Add runs 4 times in the calc binary (TestAdd twice through
		// GeneratedDouble, TestExternal, ExampleAdd) and once in the extonly
		// binary: the counts of every binary are summed.
		for _, b := range doc.Coverage.Blocks {
			if b.File == "calc/calc.go" && b.StartLine == 5 && b.Count != 5 {
				t.Errorf("Add ran %d times, want 5", b.Count)
			}
		}
	})

	t.Run("ids match analyze", func(t *testing.T) {
		analysis := analyze(t, dir)
		functions := map[string]protocol.Function{}
		for _, f := range analysis.Functions {
			functions[f.ID] = f
		}
		for _, test := range doc.Tests {
			if test.Function == nil {
				t.Errorf("%s: no function", test.Name)
				continue
			}
			f, ok := functions[*test.Function]
			switch {
			case !ok:
				t.Errorf("%s: function %s is not among analyze's functions", test.Name, *test.Function)
			case !f.Test:
				t.Errorf("%s: function %s is not test code for analyze", test.Name, f.ID)
			case f.Container != test.Container || f.File != str(test.File) || test.Line == nil || f.StartLine != *test.Line:
				t.Errorf("%s: test says %s %s:%v, analyze says %s %s:%d", test.Name, test.Container, str(test.File), test.Line, f.Container, f.File, f.StartLine)
			}
		}
		containers := map[string]bool{}
		for _, f := range analysis.Functions {
			containers[f.Container] = true
		}
		for _, p := range doc.Packages {
			if !containers[p.Container] {
				t.Errorf("package container %s is not one of analyze's", p.Container)
			}
		}
	})
}

func TestTestCommandWithoutRace(t *testing.T) {
	doc, _ := runTests(t, fixtureCopy(t, "testrun"), "--no-race")
	if doc.Race {
		t.Error("race is true with --no-race")
	}
	if len(doc.Races) != 0 {
		t.Errorf("races without the detector: %+v", doc.Races)
	}
	if !slices.ContainsFunc(doc.Diagnostics, func(d string) bool { return strings.Contains(d, "--no-race") }) {
		t.Errorf("no diagnostic says why the race detector was off: %q", doc.Diagnostics)
	}
	if got := testByName(t, doc, "TestCounterRace"); got.Outcome != protocol.OutcomePass {
		t.Errorf("TestCounterRace without the detector: %s", got.Outcome)
	}
	if got := str(testByName(t, doc, "TestDiv/by_zero").Command); got != `go test -run '^TestDiv$/^by_zero$' ./calc` {
		t.Errorf("command without the detector: %s", got)
	}
	if got := str(packageByContainer(t, doc, "calc").Command); got != "go test ./calc" {
		t.Errorf("package command without the detector: %s", got)
	}
}

// The commands of a tree with several modules, with and without go.work,
// run from the root of the tree, and run what they say.
func TestCommandsRunFromTheRoot(t *testing.T) {
	if testing.Short() {
		t.Skip("runs go test")
	}
	files := map[string]string{
		"a/go.mod":        "module example.com/a\n\ngo 1.24\n",
		"a/a.go":          "package a\n\nfunc A() int { return 1 }\n",
		"a/a_test.go":     "package a\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {\n\tt.Run(\"one (x)\", func(t *testing.T) {})\n\tt.Run(\"two\", func(t *testing.T) { t.Fatal(\"two ran\") })\n}\n",
		"b/go.mod":        "module example.com/b\n\ngo 1.24\n",
		"b/sub/sub.go":    "package sub\n\nfunc B() int { return 2 }\n",
		"b/sub/b_test.go": "package sub\n\nimport \"testing\"\n\nfunc TestB(t *testing.T) {}\n",
	}
	for _, workspace := range []bool{false, true} {
		dir := t.TempDir()
		for name, content := range files {
			writeFile(t, filepath.Join(dir, name), content)
		}
		// Without go.work, go test runs in each module; with it, at the root.
		inA, inB, pkgA, pkgB := "-C a ", "-C b ", ".", "./sub"
		if workspace {
			writeFile(t, filepath.Join(dir, "go.work"), "go 1.24\n\nuse (\n\t./a\n\t./b\n)\n")
			inA, inB, pkgA, pkgB = "", "", "./a", "./b/sub"
		}
		doc, _ := runTests(t, dir, "--no-cover")
		sub := testByName(t, doc, "TestA/one_(x)")
		if !slices.Equal(sub.Hierarchy, []string{"TestA", "one_(x)"}) {
			t.Errorf("workspace=%v: hierarchy %q", workspace, sub.Hierarchy)
		}
		for _, tc := range []struct {
			got  *string
			want string
		}{
			{sub.Command, "go test " + inA + `-race -run '^TestA$/^one_\(x\)$' ` + pkgA},
			{testByName(t, doc, "TestB").Command, "go test " + inB + `-race -run '^TestB$' ` + pkgB},
			{packageByContainer(t, doc, "b/sub").Command, "go test " + inB + "-race " + pkgB},
		} {
			if str(tc.got) != tc.want {
				t.Errorf("workspace=%v: got %s, want %s", workspace, str(tc.got), tc.want)
				continue
			}
			// The subtest that fails is not run.
			cmd := exec.Command("sh", "-c", *tc.got)
			cmd.Dir = dir
			if out, err := cmd.CombinedOutput(); err != nil || !strings.Contains(string(out), "ok") {
				t.Errorf("workspace=%v: %s: %v\n%s", workspace, *tc.got, err, out)
			}
		}
	}
}

// --cache-dir is accepted by prepare, test and check, as "--cache-dir v"
// and "--cache-dir=v", and made absolute.
func TestCacheDirFlag(t *testing.T) {
	a := newApp(&bytes.Buffer{}, &bytes.Buffer{})
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	for _, args := range [][]string{
		{"--dir", ".", "--cache-dir", "cache/x"},
		{"--dir", ".", "--cache-dir=cache/x"},
		{"--cache-dir", filepath.Join(wd, "cache", "x"), "--dir", "."},
	} {
		for _, name := range []string{"prepare", "test", "check"} {
			flags := a.newFlags(name, time.Minute)
			flags.withCacheDir()
			if !a.parse(flags, args) {
				t.Errorf("%s %q: rejected", name, args)
				continue
			}
			if got, want := flags.cacheDirValue(), filepath.Join(wd, "cache", "x"); got != want {
				t.Errorf("%s %q: --cache-dir %s, want %s", name, args, got, want)
			}
		}
	}
	flags := a.newFlags("test", time.Minute)
	flags.withCacheDir()
	if !a.parse(flags, []string{"--dir", "."}) || flags.cacheDirValue() != "" {
		t.Errorf("without --cache-dir: %q", flags.cacheDirValue())
	}
	// analyze does not take it.
	if flags := a.newFlags("analyze", time.Minute); a.parse(flags, []string{"--dir", ".", "--cache-dir", "x"}) {
		t.Error("analyze accepted --cache-dir")
	}
}

// With the user's build cache off, the build cache lives in --cache-dir:
// prepare reports it, and test and check use it.
func TestCacheDirHoldsTheBuildCacheWhenGOCACHEIsOff(t *testing.T) {
	if testing.Short() {
		t.Skip("builds the standard library into an empty build cache")
	}
	dir := t.TempDir()
	writeFile(t, filepath.Join(dir, "go.mod"), "module example.com/c\n\ngo 1.24\n")
	writeFile(t, filepath.Join(dir, "c.go"), "package c\n\nfunc One() int { return 1 }\n")
	writeFile(t, filepath.Join(dir, "c_test.go"), "package c\n\nimport \"testing\"\n\nfunc TestOne(t *testing.T) {\n\tif One() != 1 {\n\t\tt.Fatal(\"not one\")\n\t}\n}\n")
	cacheDir := filepath.Join(t.TempDir(), "cache")
	t.Setenv("GOCACHE", "off")

	if _, _, code := runCLI(t, "prepare", "--dir", dir); code != exitFailed {
		t.Errorf("prepare without a usable build cache nor --cache-dir: exit %d", code)
	}
	out, stderr, code := runCLI(t, "prepare", "--dir", dir, "--cache-dir", cacheDir)
	if code != 0 {
		t.Fatalf("prepare: exit %d: %s", code, stderr)
	}
	var prepared protocol.Prepare
	if err := json.Unmarshal(out, &prepared); err != nil {
		t.Fatal(err)
	}
	cache := filepath.Join(cacheDir, "go-build")
	if !slices.Equal(prepared.Writable, []string{cache}) {
		t.Errorf("writable %q, want %s", prepared.Writable, cache)
	}

	doc, _ := runTests(t, dir, "--cache-dir", cacheDir, "--no-race", "--no-cover")
	if got := testByName(t, doc, "TestOne"); got.Outcome != protocol.OutcomePass {
		t.Errorf("TestOne: %+v; diagnostics %q", got, doc.Diagnostics)
	}
	if entries, err := os.ReadDir(cache); err != nil || len(entries) == 0 {
		t.Errorf("the build cache in --cache-dir is empty: %v", err)
	}

	out, stderr, code = runCLI(t, "check", "--dir", dir, "--timeout", "2m", "--cache-dir", cacheDir)
	if code != 0 {
		t.Fatalf("check: exit %d: %s", code, stderr)
	}
	var checked protocol.CheckRun
	if err := json.Unmarshal(out, &checked); err != nil {
		t.Fatal(err)
	}
	if vet := checked.Checks[0]; vet.Status != protocol.CheckClean {
		t.Errorf("vet with the build cache in --cache-dir: %+v %s", vet, str(vet.Message))
	}
}

// A selection runs only what it names: a package whole, some tests of
// another, some subtests of a test, and reports only those, with the
// progress written along the way.
func TestTestCommandRunsASelection(t *testing.T) {
	dir := fixtureCopy(t, "testrun")
	files := t.TempDir()
	selection := filepath.Join(files, "selection.json")
	progressFile := filepath.Join(files, "progress.json")
	writeFile(t, selection, `{"packages":[
		{"package":"broken","tests":null},
		{"package":"calc","tests":[
			{"name":"TestAdd","subtests":null},
			{"name":"TestSkipped","subtests":null},
			{"name":"TestDiv","subtests":["by_one"]}
		]},
		{"package":"gone","tests":null}
	]}`)
	requireSchema(t, testsSchemaPath+"#/$defs/selection", []byte(readFile(t, selection)))

	doc, out := runTests(t, dir, "--selection", selection, "--progress", progressFile)
	requireSchema(t, testsSchemaPath, out)

	names := []string{}
	for _, test := range doc.Tests {
		names = append(names, test.Package+":"+test.Name+":"+test.Outcome)
	}
	slices.Sort(names)
	want := []string{"calc:TestAdd:pass", "calc:TestDiv/by_one:pass", "calc:TestDiv:pass", "calc:TestSkipped:skip"}
	if !slices.Equal(names, want) {
		t.Errorf("tests run: %q, want %q", names, want)
	}
	containers := []string{}
	for _, p := range doc.Packages {
		containers = append(containers, p.Container+":"+p.Outcome)
	}
	slices.Sort(containers)
	if want := []string{"broken:build_failed", "calc:pass"}; !slices.Equal(containers, want) {
		t.Errorf("packages: %q, want %q (one entry per package, however many go test runs it took)", containers, want)
	}
	if !slices.ContainsFunc(doc.Diagnostics, func(d string) bool { return strings.Contains(d, "gone was not found") }) {
		t.Errorf("no diagnostic for the package not found: %q", doc.Diagnostics)
	}
	if doc.Coverage == nil {
		t.Errorf("a partial run measures coverage too: %q", doc.Diagnostics)
	}

	data := []byte(readFile(t, progressFile))
	requireSchema(t, testsSchemaPath+"#/$defs/progress", data)
	var p protocol.Progress
	if err := json.Unmarshal(data, &p); err != nil {
		t.Fatal(err)
	}
	// broken and calc's whole tests share nothing: one go test for the
	// packages run whole, one for calc's whole tests, one for TestDiv's
	// subtest; each reports one package end.
	if p.PackagesDone != 3 || p.PackagesTotal != 3 || p.Tests != 3 || p.Failed != 0 || p.Current != nil {
		t.Errorf("final progress: %+v", p)
	}
}

func TestTestCommandWithoutCoverage(t *testing.T) {
	doc, _ := runTests(t, fixtureCopy(t, "testrun"), "--no-cover")
	if doc.Coverage != nil {
		t.Error("coverage was measured with --no-cover")
	}
	if !slices.ContainsFunc(doc.Diagnostics, func(d string) bool { return strings.Contains(d, "--no-cover") }) {
		t.Errorf("no diagnostic says why coverage was not measured: %q", doc.Diagnostics)
	}
	if !doc.Race || len(doc.Tests) != 10 {
		t.Errorf("race %v, %d tests", doc.Race, len(doc.Tests))
	}
}

func TestPrepareCommand(t *testing.T) {
	dir := fixtureCopy(t, "testrun")
	before := hashTree(t, dir)
	out, stderr, code := runCLI(t, "prepare", "--dir", dir)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	requireSchema(t, testsSchemaPath+"#/$defs/prepare", out)
	var doc protocol.Prepare
	if err := json.Unmarshal(out, &doc); err != nil {
		t.Fatal(err)
	}
	cache, err := os.UserCacheDir()
	if err != nil {
		t.Fatal(err)
	}
	if len(doc.Writable) != 1 || !filepath.IsAbs(doc.Writable[0]) {
		t.Fatalf("writable: %q", doc.Writable)
	}
	if env := os.Getenv("GOCACHE"); env != "" && doc.Writable[0] != env {
		t.Errorf("writable %s is not GOCACHE %s", doc.Writable[0], env)
	} else if env == "" && !strings.HasPrefix(doc.Writable[0], cache) {
		t.Errorf("writable %s is not the default build cache under %s", doc.Writable[0], cache)
	}
	if hashTree(t, dir) != before {
		t.Error("prepare changed the tree")
	}
}

// A dependency through a local replace needs no network, so the offline run
// of a module with a dependency can be tested.
func TestOfflineRunWithADependency(t *testing.T) {
	root := t.TempDir()
	writeFile(t, filepath.Join(root, "dep", "go.mod"), "module example.com/dep\n\ngo 1.24\n")
	writeFile(t, filepath.Join(root, "dep", "dep.go"), "package dep\n\nfunc One() int { return 1 }\n")
	app := filepath.Join(root, "app")
	writeFile(t, filepath.Join(app, "go.mod"), "module example.com/app\n\ngo 1.24\n\nrequire example.com/dep v0.0.0\n\nreplace example.com/dep => ../dep\n")
	writeFile(t, filepath.Join(app, "app.go"), "package app\n\nimport \"example.com/dep\"\n\nfunc Two() int { return dep.One() + 1 }\n")
	writeFile(t, filepath.Join(app, "app_test.go"), "package app\n\nimport \"testing\"\n\nfunc TestTwo(t *testing.T) {\n\tif Two() != 2 {\n\t\tt.Fatal(\"not two\")\n\t}\n}\n")
	before := hashTree(t, root)
	if _, stderr, code := runCLI(t, "prepare", "--dir", app); code != 0 {
		t.Fatalf("prepare: exit %d: %s", code, stderr)
	}
	if hashTree(t, root) != before {
		t.Error("prepare changed the tree")
	}
	doc, _ := runTests(t, app)
	if got := testByName(t, doc, "TestTwo"); got.Outcome != protocol.OutcomePass || str(got.Function) != "example.com/app.TestTwo" {
		t.Errorf("TestTwo: %+v", got)
	}
	for _, b := range doc.Coverage.Blocks {
		if !strings.HasPrefix(b.File, "app.go") {
			t.Errorf("coverage outside the tree: %+v", b)
		}
	}
}

// A package stuck before its tests start (where go test's own -timeout does
// not apply) is stopped by the runner's deadline, and the run still reports.
func TestTestCommandEnforcesTheTimeout(t *testing.T) {
	dir := t.TempDir()
	writeFile(t, filepath.Join(dir, "go.mod"), "module example.com/stuck\n\ngo 1.24\n")
	writeFile(t, filepath.Join(dir, "stuck_test.go"), "package stuck\n\nimport (\n\t\"os\"\n\t\"testing\"\n\t\"time\"\n)\n\n"+
		"func TestMain(m *testing.M) {\n\ttime.Sleep(time.Hour)\n\tos.Exit(m.Run())\n}\n\nfunc TestNever(t *testing.T) {}\n")
	start := time.Now()
	out, stderr, code := runCLI(t, "test", "--dir", dir, "--timeout", "3s", "--offline")
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	if elapsed := time.Since(start); elapsed > 10*time.Second {
		t.Errorf("the run took %s with a 3s timeout", elapsed)
	}
	requireSchema(t, testsSchemaPath, out)
	var doc protocol.TestRun
	if err := json.Unmarshal(out, &doc); err != nil {
		t.Fatal(err)
	}
	if p := packageByContainer(t, &doc, "stuck"); p.Outcome != protocol.OutcomeFail || !strings.Contains(str(p.Output), "did not finish") {
		t.Errorf("stuck package: %+v", p)
	}
	if doc.Coverage != nil || !slices.ContainsFunc(doc.Diagnostics, func(d string) bool { return strings.Contains(d, "stopped after 3s") }) {
		t.Errorf("coverage %v, diagnostics %q", doc.Coverage, doc.Diagnostics)
	}
}

func notInstalled(context.Context, string) (string, error) {
	return "", errors.New("staticcheck is not installed")
}

func runCheck(t *testing.T, find checks.Finder, dir string) (*protocol.CheckRun, []byte) {
	t.Helper()
	log := logging.Discard()
	var stdout, stderr bytes.Buffer
	app := New(&stdout, &stderr, log, analyzer.New(log, entrypoints.Default()), testrun.New(log), checks.New(log, find), nil)
	if code := app.Run([]string{"check", "--dir", dir, "--timeout", "2m"}); code != 0 {
		t.Fatalf("exit %d: %s", code, stderr.String())
	}
	var doc protocol.CheckRun
	if err := json.Unmarshal(stdout.Bytes(), &doc); err != nil {
		t.Fatal(err)
	}
	return &doc, stdout.Bytes()
}

func TestCheckCommand(t *testing.T) {
	dir := t.TempDir()
	copyTree(t, "testdata", dir)
	before := hashTree(t, dir)

	t.Run("findings", func(t *testing.T) {
		doc, out := runCheck(t, notInstalled, filepath.Join(dir, "lint"))
		requireSchema(t, checksSchemaPath, out)
		if len(doc.Checks) != 2 || doc.Checks[0].Tool != checks.ToolVet || doc.Checks[1].Tool != checks.ToolStaticcheck {
			t.Fatalf("checks: %+v", doc.Checks)
		}
		vet := doc.Checks[0]
		if vet.Status != protocol.CheckFindings || vet.Message != nil || len(vet.Findings) != 1 {
			t.Fatalf("vet: %+v", vet)
		}
		if f := vet.Findings[0]; f.File != "report/report.go" || f.Line != 7 || str(f.Rule) != "printf" || !strings.Contains(f.Message, "wrong type") ||
			f.Severity != protocol.SeverityError {
			t.Errorf("vet finding: %+v", f)
		}
		if sc := doc.Checks[1]; sc.Status != protocol.CheckUnavailable || sc.Message == nil || len(sc.Findings) != 0 {
			t.Errorf("staticcheck: %+v", sc)
		}
	})

	t.Run("clean", func(t *testing.T) {
		doc, _ := runCheck(t, notInstalled, filepath.Join(dir, "basic"))
		if vet := doc.Checks[0]; vet.Status != protocol.CheckClean || len(vet.Findings) != 0 || vet.Message != nil {
			t.Errorf("vet: %+v", vet)
		}
	})

	t.Run("code that does not compile", func(t *testing.T) {
		doc, out := runCheck(t, notInstalled, filepath.Join(dir, "broken"))
		requireSchema(t, checksSchemaPath, out)
		vet := doc.Checks[0]
		if vet.Status != protocol.CheckFailed || !strings.Contains(str(vet.Message), "bad/bad.go:6:14") || strings.Contains(str(vet.Message), "\n") {
			t.Errorf("vet: %+v %q", vet, str(vet.Message))
		}
	})

	t.Run("staticcheck", func(t *testing.T) {
		if _, err := checks.DefaultFinder(t.Context(), checks.ToolStaticcheck); err != nil {
			t.Skip("staticcheck is not installed")
		}
		doc, out := runCheck(t, checks.DefaultFinder, filepath.Join(dir, "lint"))
		requireSchema(t, checksSchemaPath, out)
		sc := doc.Checks[1]
		if sc.Status != protocol.CheckFindings || !slices.ContainsFunc(sc.Findings, func(f protocol.Finding) bool {
			return f.File == "report/report.go" && f.Line == 11 && str(f.Rule) == "SA4006"
		}) {
			t.Errorf("staticcheck: %+v", sc)
		}
		doc, _ = runCheck(t, checks.DefaultFinder, filepath.Join(dir, "broken"))
		if sc := doc.Checks[1]; sc.Status != protocol.CheckFailed || !strings.Contains(str(sc.Message), "bad/bad.go:6:14") {
			t.Errorf("staticcheck on code that does not compile: %+v", sc)
		}
	})

	if hashTree(t, dir) != before {
		t.Error("check wrote inside the tree")
	}
}

// A stand-in staticcheck prints what the real one prints, so that reading
// its output is tested where staticcheck is not installed.
func TestCheckReadsStaticcheckOutput(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the stand-in is a shell script")
	}
	dir := fixtureCopy(t, "lint")
	fake := filepath.Join(t.TempDir(), "staticcheck")
	writeFile(t, fake, `#!/bin/sh
echo '{"code":"SA4006","severity":"error","location":{"file":"'"$PWD"'/report/report.go","line":11,"column":2},"message":"this value of total is never used"}'
echo '{"code":"U1000","severity":"error","location":{"file":"'"$PWD"'/report/report.go","line":14,"column":6},"message":"func unused is unused"}'
echo '{"code":"S1008","severity":"warning","location":{"file":"'"$PWD"'/report/report.go","line":15,"column":2},"message":"should use return x"}'
echo '{"code":"ST1000","severity":"ignored","location":{"file":"'"$PWD"'/report/report.go","line":1,"column":1},"message":"ignored"}'
echo '{"code":"SA1000","severity":"error","location":{"file":"/elsewhere/x.go","line":1,"column":1},"message":"outside the tree"}'
printf '%s\n' '{"code":"compile","severity":"error","location":{"file":"","line":0,"column":0},"message":"# example.com/lint/other\nother/other.go:3:1: syntax error"}'
echo 'not json'
exit 1
`)
	if err := os.Chmod(fake, 0o755); err != nil {
		t.Fatal(err)
	}
	doc, out := runCheck(t, func(context.Context, string) (string, error) { return fake, nil }, dir)
	requireSchema(t, checksSchemaPath, out)
	sc := doc.Checks[1]
	if sc.Status != protocol.CheckFailed || str(sc.Message) != "example.com/lint/other does not compile: other/other.go:3:1: syntax error" {
		t.Errorf("staticcheck: %+v %q", sc, str(sc.Message))
	}
	var findings []string
	for _, f := range sc.Findings {
		findings = append(findings, fmt.Sprintf("%s:%d %s %s", f.File, f.Line, str(f.Rule), f.Severity))
	}
	// The severity comes from the check's category, not from what
	// staticcheck prints.
	if want := []string{"report/report.go:11 SA4006 error", "report/report.go:14 U1000 warning", "report/report.go:15 S1008 info"}; !slices.Equal(findings, want) {
		t.Errorf("findings: %q, want %q", findings, want)
	}
	if doc.Checks[0].Findings[0].Severity != protocol.SeverityError {
		t.Errorf("vet finding: %+v", doc.Checks[0].Findings[0])
	}

	writeFile(t, fake, "#!/bin/sh\necho 'staticcheck: internal error' >&2\nexit 2\n")
	doc, _ = runCheck(t, func(context.Context, string) (string, error) { return fake, nil }, dir)
	if sc := doc.Checks[1]; sc.Status != protocol.CheckFailed || str(sc.Message) != "staticcheck failed: staticcheck: internal error" {
		t.Errorf("crashed staticcheck: %+v %q", sc, str(sc.Message))
	}
}

func TestNewCommandsUsage(t *testing.T) {
	for _, args := range [][]string{
		{"test"},
		{"test", "--dir", "testdata/testrun", "--timeout", "0s"},
		{"test", "--dir", "testdata/testrun", "--selection", filepath.Join(t.TempDir(), "missing.json")},
		{"check", "--dir", "testdata/lint", "extra"},
		{"prepare"},
		{"unknown"},
	} {
		if _, _, code := runCLI(t, args...); code != exitUsage {
			t.Errorf("%q: exit %d, want %d", args, code, exitUsage)
		}
	}
	if _, _, code := runCLI(t, "test", "--dir", t.TempDir(), "--timeout", "1m"); code != exitFailed {
		t.Error("a tree without go.mod must fail the test command")
	}
}
