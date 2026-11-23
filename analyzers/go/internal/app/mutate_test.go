package app

import (
	"encoding/json"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/rootless-dev/difflens/analyzers/go/internal/mutate"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

const mutationSchemaPath = "../../../protocol/mutation.schema.json"

// mutateProject copies the mutate package's test project, as the core gives
// the command a throwaway copy of the tree.
func mutateProject(t *testing.T) string {
	t.Helper()
	dir := filepath.Join(t.TempDir(), "project")
	copyTree(t, "../mutate/testdata/project", dir)
	return dir
}

func TestMutateCommandSamples(t *testing.T) {
	dir := mutateProject(t)
	before := hashTree(t, dir)
	progress := filepath.Join(t.TempDir(), "progress.json")
	out, stderr, code := runCLI(t, "mutate", "--dir", dir, "--timeout", "2m",
		"--changed", filepath.Join(dir, "changed.json"), "--coverage", filepath.Join(dir, "coverage.json"),
		"--max-mutants", "4", "--jobs", "2", "--min-mutant-timeout", "1s", "--progress", progress,
		"--tools", filepath.Join(t.TempDir(), "tools"), "--offline")
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	requireSchema(t, mutationSchemaPath, out)
	var doc protocol.MutationRun
	if err := json.Unmarshal(out, &doc); err != nil {
		t.Fatal(err)
	}
	if doc.Language != "go" || doc.Runner != (protocol.Analyzer{Name: Name, Version: Version}) ||
		doc.Engine != (protocol.Analyzer{Name: mutate.EngineName, Version: Version}) {
		t.Errorf("header: %+v %+v %+v", doc.Language, doc.Runner, doc.Engine)
	}
	// Round-robin: the first mutant of each function, by file and start,
	// until four are taken: Withdraw, Half, CountDown, Double.
	var got []string
	for _, m := range doc.Mutants {
		got = append(got, m.File+":"+m.Original+"->"+m.Mutated+" "+m.Status)
	}
	want := []string{
		"bank/bank.go:>=->> survived",
		"broken/broken.go:n / 2->0 error",
		"calc/calc.go:>->>= killed",
		"calc/calc.go:Map(xs, func(x int) int { return x * 2 })->nil killed",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Errorf("mutants:\n got %q\nwant %q", got, want)
	}
	s := doc.Summary
	if s.Candidates != 20 || !s.Sampled || s.MaxMutants != 4 || s.Killed != 2 || s.Survived != 1 || s.Error != 1 {
		t.Errorf("summary %+v", s)
	}
	requireSchema(t, mutationSchemaPath+"#/$defs/progress", []byte(readFile(t, progress)))
	if hashTree(t, dir) != before {
		t.Error("mutate wrote inside the tree")
	}
}

func TestMutateUsageErrors(t *testing.T) {
	dir := t.TempDir()
	file := func(content string) string {
		path := filepath.Join(t.TempDir(), "input.json")
		writeFile(t, path, content)
		return path
	}
	changed := file(`{"protocol": 1, "files": []}`)
	coverage := file(`{"protocol": 1, "blocks": []}`)
	base := []string{"--dir", dir, "--changed", changed, "--coverage", coverage}
	for name, args := range map[string][]string{
		"no --coverage":          {"--dir", dir, "--changed", changed},
		"no --changed":           {"--dir", dir, "--coverage", coverage},
		"missing coverage file":  {"--dir", dir, "--changed", changed, "--coverage", filepath.Join(dir, "missing.json")},
		"coverage not JSON":      {"--dir", dir, "--changed", changed, "--coverage", file(`blocks`)},
		"coverage protocol 2":    {"--dir", dir, "--changed", changed, "--coverage", file(`{"protocol": 2, "blocks": []}`)},
		"coverage no blocks":     {"--dir", dir, "--changed", changed, "--coverage", file(`{"protocol": 1}`)},
		"coverage unknown field": {"--dir", dir, "--changed", changed, "--coverage", file(`{"protocol": 1, "blocks": [], "x": 1}`)},
		"coverage reversed block": {"--dir", dir, "--changed", changed, "--coverage",
			file(`{"protocol": 1, "blocks": [{"file": "a.go", "startLine": 5, "endLine": 4, "statements": 1, "count": 1}]}`)},
		"--max-mutants 0":           slices.Concat(base, []string{"--max-mutants", "0"}),
		"--jobs -1":                 slices.Concat(base, []string{"--jobs", "-1"}),
		"--min-mutant-timeout 0s":   slices.Concat(base, []string{"--min-mutant-timeout", "0s"}),
		"positional argument":       slices.Concat(base, []string{"extra"}),
		"changed of the wrong kind": {"--dir", dir, "--changed", coverage, "--coverage", coverage},
	} {
		t.Run(name, func(t *testing.T) {
			out, stderr, code := runCLI(t, append([]string{"mutate"}, args...)...)
			if code != exitUsage || len(out) != 0 || stderr == "" {
				t.Errorf("exit %d, stdout %q, stderr %q", code, out, stderr)
			}
		})
	}
	// A tree without go.mod is a failure of the command, not of its input.
	if _, _, code := runCLI(t, append([]string{"mutate"}, base...)...); code != exitFailed {
		t.Errorf("a tree without a module: exit %d", code)
	}
	// The exclude of --changed is for dupes: mutate accepts it and ignores
	// it, so the input is fine and the same failure follows.
	withExclude := file(`{"protocol": 1, "exclude": {"dirs": ["locales"], "extensions": [".po"]}, "files": []}`)
	if _, stderr, code := runCLI(t, "mutate", "--dir", dir, "--changed", withExclude, "--coverage", coverage); code != exitFailed {
		t.Errorf("a --changed file with exclude: exit %d, stderr %q", code, stderr)
	}
}

func TestReadChangedKeepsTheExclude(t *testing.T) {
	path := filepath.Join(t.TempDir(), "changed.json")
	writeFile(t, path, `{"protocol": 1, "exclude": {"dirs": ["locales"], "extensions": [".po"]},
		"files": [{"path": "./a.go", "added": [[2, 3]]}]}`)
	added, exclude, err := readChanged(path)
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(added["a.go"], [][2]int{{2, 3}}) || !slices.Equal(exclude.Dirs, []string{"locales"}) ||
		!slices.Equal(exclude.Extensions, []string{".po"}) {
		t.Errorf("added %v exclude %+v", added, exclude)
	}

	writeFile(t, path, `{"protocol": 1, "files": []}`)
	if _, exclude, err := readChanged(path); err != nil || exclude.Dirs != nil || exclude.Extensions != nil {
		t.Errorf("without exclude: %+v %v", exclude, err)
	}
}

func TestPrepareAcceptsTools(t *testing.T) {
	dir := mutateProject(t)
	out, stderr, code := runCLI(t, "prepare", "--dir", dir, "--tools", filepath.Join(t.TempDir(), "tools"))
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	requireSchema(t, testsSchemaPath+"#/$defs/prepare", out)
}

func TestUsageListsMutate(t *testing.T) {
	_, stderr, code := runCLI(t, "help")
	if code != exitUsage || !strings.Contains(stderr, " mutate --dir <source tree> --changed <file> --coverage <file>") {
		t.Errorf("exit %d, usage %q", code, stderr)
	}
}
