package mutate

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/santhosh-tekuri/jsonschema/v6"

	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
	"github.com/rootless-dev/difflens/analyzers/go/internal/testrun"
)

const schemaPath = "../../../protocol/mutation.schema.json"

// project copies testdata/project, as the core gives the command a
// throwaway copy of the tree, and returns it with the options of a run on
// its changed.json and coverage.json.
func project(t *testing.T) Options {
	t.Helper()
	dir := filepath.Join(t.TempDir(), "project")
	err := filepath.WalkDir("testdata/project", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel("testdata/project", path)
		if d.IsDir() {
			return os.MkdirAll(filepath.Join(dir, rel), 0o755)
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		return os.WriteFile(filepath.Join(dir, rel), data, 0o644)
	})
	if err != nil {
		t.Fatal(err)
	}
	var changed protocol.Changed
	var coverage protocol.MutationCoverage
	decodeFile(t, filepath.Join(dir, "changed.json"), &changed)
	decodeFile(t, filepath.Join(dir, "coverage.json"), &coverage)
	added := map[string][][2]int{}
	for _, f := range changed.Files {
		for _, r := range f.Added {
			added[f.Path] = append(added[f.Path], [2]int{r[0], r[1]})
		}
	}
	return Options{
		Dir: dir, Timeout: 2 * time.Minute, Added: added, Coverage: coverage.Blocks,
		MaxMutants: DefaultMaxMutants, MinMutantTimeout: time.Second,
		Engine: protocol.Analyzer{Name: EngineName, Version: "test"},
	}
}

func decodeFile(t *testing.T, file string, v any) {
	t.Helper()
	data, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(data, v); err != nil {
		t.Fatalf("%s: %v", file, err)
	}
}

func runMutate(t *testing.T, opts Options) *Result {
	t.Helper()
	log := logging.Discard()
	res, err := New(log, testrun.New(log)).Run(context.Background(), opts)
	if err != nil {
		t.Fatal(err)
	}
	return res
}

// line is a mutant as the tests compare them.
func line(m protocol.Mutant) string {
	killedBy, function := "-", "-"
	if m.KilledBy != nil {
		killedBy = *m.KilledBy
	}
	if m.Function != nil {
		function = strings.TrimPrefix(*m.Function, "example.com/mutate/")
	}
	return fmt.Sprintf("%s:%d:%d %s -> %s %s %s %s %s", m.File, m.Line, m.Column, m.Original, m.Mutated, m.Operator, m.Status, killedBy, function)
}

// wantMutants are the mutants of testdata/project. Not there: the log call
// and `return false` of Withdraw (logging; already the zero value), Map (not
// added), Unused (added, never run) and the added lines of calc_test.go.
var wantMutants = []string{
	"bank/bank.go:13:15 >= -> < relational_negation killed TestWithdraw/enough bank.(*Account).Withdraw",
	"bank/bank.go:13:15 >= -> > relational_boundary survived - bank.(*Account).Withdraw",
	"bank/bank.go:14:13 -= -> += increment killed TestWithdraw/enough bank.(*Account).Withdraw",
	`bank/bank.go:15:3 a.record("withdraw") ->  call_removal survived - bank.(*Account).Withdraw`,
	"bank/bank.go:16:10 true -> false return_value killed TestWithdraw/enough bank.(*Account).Withdraw",
	"broken/broken.go:5:9 n / 2 -> 0 return_value error - broken.Half",
	"broken/broken.go:5:11 / -> * arithmetic error - broken.Half",
	"calc/calc.go:6:16 > -> <= relational_negation killed TestCountDown calc.CountDown",
	"calc/calc.go:6:16 > -> >= relational_boundary killed TestCountDown calc.CountDown",
	"calc/calc.go:6:22 -- -> ++ increment timeout - calc.CountDown",
	"calc/calc.go:7:8 ++ -> -- increment killed TestCountDown calc.CountDown",
	"calc/calc.go:9:9 steps -> 0 return_value killed TestCountDown calc.CountDown",
	"calc/calc.go:14:9 Map(xs, func(x int) int { return x * 2 }) -> nil return_value killed TestDouble calc.Double",
	"calc/calc.go:14:42 x * 2 -> 0 return_value killed TestDouble calc.Double$1",
	"calc/calc.go:14:44 * -> / arithmetic killed TestDouble calc.Double$1",
	"calc/calc.go:28:28 > -> <= relational_negation killed TestTag calc.Tag",
	"calc/calc.go:28:28 > -> >= relational_boundary killed TestTag calc.Tag",
	`calc/calc.go:29:10 name -> "" return_value killed TestTag calc.Tag`,
	"notests/notests.go:7:9 n + 1 -> 0 return_value no_coverage - notests.Inc",
	"notests/notests.go:7:11 + -> - arithmetic no_coverage - notests.Inc",
}

func TestRunAndReuse(t *testing.T) {
	opts := project(t)
	opts.CacheDir = t.TempDir()
	opts.Progress = filepath.Join(t.TempDir(), "progress.json")
	res := runMutate(t, opts)

	var got []string
	for _, m := range res.Mutants {
		got = append(got, line(m))
	}
	if want := slices.Sorted(slices.Values(wantMutants)); !slices.Equal(slices.Sorted(slices.Values(got)), want) {
		t.Errorf("mutants:\n got %s\nwant %s", strings.Join(got, "\n     "), strings.Join(want, "\n     "))
	}
	requireReportOrder(t, res.Mutants)
	requireOriginalText(t, opts.Dir, res.Mutants)
	s := res.Summary
	if s.Candidates != 20 || s.Sampled || s.MaxMutants != 200 || s.Killed != 13 || s.Survived != 2 || s.Timeout != 1 ||
		s.Error != 2 || s.NoCoverage != 2 || s.Cached != 0 || res.Tested != 16 {
		t.Errorf("summary %+v, tested %d", s, res.Tested)
	}
	if len(res.Diagnostics) != 2 ||
		!strings.Contains(res.Diagnostics[0], "the tests of example.com/mutate/broken fail without any mutant (TestHalf), so its 2 mutants were not tested") ||
		!strings.Contains(res.Diagnostics[1], "example.com/mutate/notests has no tests of its own") {
		t.Errorf("diagnostics: %q", res.Diagnostics)
	}
	for _, m := range res.Mutants {
		if m.Operator == protocol.OperatorRelationalBoundary && m.File == "bank/bank.go" && m.Description != "replace >= with > in Account.Withdraw" {
			t.Errorf("description %q", m.Description)
		}
		if m.Status == protocol.MutantTimeout && m.DurationMs != 1000 {
			t.Errorf("the timeout ran %d ms, not its limit", m.DurationMs)
		}
	}
	requireSchema(t, schemaPath, document(res))
	progress, err := os.ReadFile(opts.Progress)
	if err != nil {
		t.Fatal(err)
	}
	requireSchema(t, schemaPath+"#/$defs/progress", progress)
	var p protocol.MutationProgress
	if err := json.Unmarshal(progress, &p); err != nil || p.Phase != protocol.PhaseMutating || p.Total != 20 || p.Done != 20 ||
		p.Killed != 13 || p.Survived != 2 || p.Timeout != 1 || p.Error != 4 || p.Current != nil {
		t.Errorf("progress %s (%v)", progress, err)
	}

	// Again: every result that depends only on the code is reused, nothing
	// is tested again, and the report is the same.
	again := runMutate(t, opts)
	if again.Tested != 0 || again.Summary.Cached != 16 {
		t.Errorf("second run: tested %d, cached %d", again.Tested, again.Summary.Cached)
	}
	for i, m := range again.Mutants {
		first := res.Mutants[i]
		wantCached := m.Status != protocol.MutantError && m.Status != protocol.MutantNoCoverage
		if m.ID != first.ID || line(m) != line(first) || m.Cached != wantCached || (m.Cached && m.DurationMs != first.DurationMs) {
			t.Errorf("second run: %s cached=%v %d ms, first %s %d ms", line(m), m.Cached, m.DurationMs, line(first), first.DurationMs)
		}
	}

	// Change Tag (a comment: same lines) and the tests of bank: their
	// mutants are tested again, the others are reused.
	edit(t, filepath.Join(opts.Dir, "calc", "calc.go"), "\t\treturn name\n", "\t\treturn name // the label\n")
	edit(t, filepath.Join(opts.Dir, "bank", "bank_test.go"), "func TestWithdraw", "// TestWithdraw withdraws.\nfunc TestWithdraw")
	third := runMutate(t, opts)
	if third.Tested != 8 || third.Summary.Cached != 8 {
		t.Errorf("after the edits: tested %d, cached %d", third.Tested, third.Summary.Cached)
	}
	for i, m := range third.Mutants {
		retested := m.File == "bank/bank.go" || (m.Function != nil && strings.HasSuffix(*m.Function, ".Tag"))
		if m.ID != res.Mutants[i].ID || line(m) != line(res.Mutants[i]) {
			t.Errorf("after the edits: %s, first %s", line(m), line(res.Mutants[i]))
		}
		if retested && m.Cached {
			t.Errorf("after the edits: %s was reused", line(m))
		}
	}
}

func edit(t *testing.T, file, old, new string) {
	t.Helper()
	data, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(data, []byte(old)) {
		t.Fatalf("%s has no %q", file, old)
	}
	if err := os.WriteFile(file, bytes.Replace(data, []byte(old), []byte(new), 1), 0o644); err != nil {
		t.Fatal(err)
	}
}

func requireReportOrder(t *testing.T, ms []protocol.Mutant) {
	t.Helper()
	ids := map[string]bool{}
	for i, m := range ms {
		if ids[m.ID] {
			t.Errorf("id %s twice", m.ID)
		}
		ids[m.ID] = true
		if i == 0 {
			continue
		}
		p := ms[i-1]
		if p.File > m.File || (p.File == m.File && (p.Line > m.Line || (p.Line == m.Line && p.Column > m.Column))) {
			t.Errorf("%s before %s", line(p), line(m))
		}
	}
}

// requireOriginalText checks that each mutant's original is the text
// between its positions, counted in code points.
func requireOriginalText(t *testing.T, dir string, ms []protocol.Mutant) {
	t.Helper()
	for _, m := range ms {
		data, err := os.ReadFile(filepath.Join(dir, m.File))
		if err != nil {
			t.Fatal(err)
		}
		lines := strings.SplitAfter(string(data), "\n")
		var text []rune
		for l := m.Line; l <= m.EndLine; l++ {
			runes := []rune(lines[l-1])
			from, to := 0, len(runes)
			if l == m.Line {
				from = m.Column - 1
			}
			if l == m.EndLine {
				to = m.EndColumn - 1
			}
			text = append(text, runes[from:to]...)
		}
		if string(text) != m.Original {
			t.Errorf("%s: the text at its position is %q", line(m), string(text))
		}
	}
}

func document(res *Result) []byte {
	data, _ := json.Marshal(protocol.MutationRun{
		Protocol: protocol.Version, Language: "go", Runner: protocol.Analyzer{Name: "test", Version: "test"},
		Engine: protocol.Analyzer{Name: EngineName, Version: "test"}, Mutants: res.Mutants, Summary: res.Summary,
		Diagnostics: res.Diagnostics,
	})
	return data
}

func requireSchema(t *testing.T, ref string, out []byte) {
	t.Helper()
	schema, err := jsonschema.NewCompiler().Compile(ref)
	if err != nil {
		t.Fatal(err)
	}
	inst, err := jsonschema.UnmarshalJSON(bytes.NewReader(out))
	if err != nil {
		t.Fatalf("not JSON: %v\n%s", err, out)
	}
	if err := schema.Validate(inst); err != nil {
		t.Errorf("does not match %s: %v", ref, err)
	}
}

func TestNoCandidates(t *testing.T) {
	opts := project(t)
	opts.Coverage = nil // nothing ran
	res := runMutate(t, opts)
	if len(res.Mutants) != 0 || res.Summary.Candidates != 0 || len(res.Diagnostics) != 0 {
		t.Errorf("result %+v", res)
	}
	requireSchema(t, schemaPath, document(res))
}

func TestRunOutOfTime(t *testing.T) {
	opts := project(t)
	opts.Timeout = time.Millisecond
	res := runMutate(t, opts)
	if len(res.Diagnostics) == 0 {
		t.Errorf("no diagnostic: %+v", res)
	}
	for _, m := range res.Mutants {
		if !m.Cached {
			t.Errorf("tested after the time ran out: %s", line(m))
		}
	}
	requireSchema(t, schemaPath, document(res))
}
