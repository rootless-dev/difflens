package mutate

import (
	"maps"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

func TestCandidateLines(t *testing.T) {
	added := map[string][][2]int{
		"a.go":      {{1, 5}, {9, 9}},
		"./b/b.go":  {{1, 3}},
		"a_test.go": {{1, 9}},
		"c.go":      {{1, 9}},
		"notes.txt": {{1, 9}},
	}
	blocks := []protocol.CoverageBlock{
		{File: "a.go", StartLine: 2, EndLine: 3, Count: 1},
		{File: "a.go", StartLine: 4, EndLine: 9, Count: 0},
		{File: "a.go", StartLine: 9, EndLine: 12, Count: 4},
		{File: "b/b.go", StartLine: 3, EndLine: 3, Count: 1},
		{File: "a_test.go", StartLine: 1, EndLine: 9, Count: 1},
		{File: "notes.txt", StartLine: 1, EndLine: 9, Count: 1},
	}
	got := candidateLines(added, blocks)
	want := map[string]map[int]bool{"a.go": {2: true, 3: true, 9: true}, "b/b.go": {3: true}}
	if !maps.EqualFunc(got, want, maps.Equal) {
		t.Errorf("got %v, want %v", got, want)
	}
}

func TestLineIndexCountsCodePoints(t *testing.T) {
	src := []byte("ab\n\tif s := \"preço\"; x > 0 {\n")
	ix := newLineIndex(src)
	if line, col := ix.position(1); line != 1 || col != 2 {
		t.Errorf("b: %d:%d", line, col)
	}
	off := slices.Index(src, '>')
	if line, col := ix.position(off); line != 2 || col != 21 { // 22 in bytes: ç takes two
		t.Errorf(">: %d:%d (byte column %d)", line, col, off-3+1)
	}
	if line, col := ix.position(len(src)); line != 3 || col != 1 {
		t.Errorf("end: %d:%d", line, col)
	}
}

func TestSampleRoundRobin(t *testing.T) {
	mk := func(file string, scope, start int) *mutant {
		return &mutant{Point: Point{Start: start}, file: file, group: groupKey{file: file, scope: scope}}
	}
	// a.go: f at 10 (3 mutants), top level (2, first at 5), g at 100 (1);
	// b.go: h at 0 (2). Sorted by file and offset, as list leaves them.
	ms := []*mutant{
		mk("a.go", -1, 5), mk("a.go", 10, 12), mk("a.go", 10, 20), mk("a.go", 10, 30),
		mk("a.go", -1, 60), mk("a.go", 100, 110), mk("b.go", 0, 3), mk("b.go", 0, 8),
	}
	got, sampled := sample(ms, 5)
	// Groups in order: a top level (at 5), f (10), g (100), h. Round one
	// takes 5, 12, 110, b:3; round two takes 60.
	want := []*mutant{ms[0], ms[1], ms[4], ms[5], ms[6]}
	if !sampled || !slices.Equal(got, want) {
		t.Errorf("sampled=%v got %v", sampled, starts(got))
	}
	if again, _ := sample(ms, 5); !slices.Equal(again, got) {
		t.Error("the sample changed")
	}
	if all, sampled := sample(ms, 8); sampled || len(all) != 8 {
		t.Errorf("no cap: sampled=%v, %d mutants", sampled, len(all))
	}
}

func starts(ms []*mutant) []string {
	var out []string
	for _, m := range ms {
		out = append(out, m.file+":"+string(rune('0'+m.Start/10))+string(rune('0'+m.Start%10)))
	}
	return out
}

func TestFailScanner(t *testing.T) {
	for _, tc := range []struct {
		out  string
		want string
	}{
		{"PASS\n", ""},
		{"--- FAIL: TestA (0.00s)\n    a_test.go:3: boom\nFAIL\n", "TestA"},
		{"--- FAIL: TestA (0.00s)\n    --- FAIL: TestA/one (0.00s)\n        --- FAIL: TestA/one/deep (0.00s)\n    --- FAIL: TestA/two (0.00s)\n--- FAIL: TestB (0.00s)\n", "TestA/one/deep"},
		{"--- FAIL: TestA (0.00s)\n--- FAIL: TestAB (0.00s)\n", "TestA"},
		{"panic: boom\n--- FAIL: TestPanics (0.00s)\n", "TestPanics"},
	} {
		s := &failScanner{}
		// Written in two pieces, split inside a line.
		half := len(tc.out) / 2
		s.Write([]byte(tc.out[:half]))
		s.Write([]byte(tc.out[half:]))
		if got := s.first(); got != tc.want {
			t.Errorf("%q: got %q, want %q", tc.out, got, tc.want)
		}
	}
}

func TestStore(t *testing.T) {
	s := newStore(t.TempDir())
	fp := hashOf("a")
	if _, ok := s.get(fp); ok {
		t.Fatal("an empty store has a result")
	}
	name := "TestA/sub"
	if err := s.put(fp, record{Status: protocol.MutantKilled, KilledBy: &name, DurationMs: 12}); err != nil {
		t.Fatal(err)
	}
	r, ok := s.get(fp)
	if !ok || r.Status != protocol.MutantKilled || r.KilledBy == nil || *r.KilledBy != name || r.DurationMs != 12 {
		t.Errorf("got %+v, %v", r, ok)
	}
	if err := os.WriteFile(s.path(fp), []byte(`{"status":"maybe"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, ok := s.get(fp); ok {
		t.Error("an unknown status is reused")
	}
	if _, ok := newStore("").get(fp); ok || newStore("").put(fp, record{}) != nil {
		t.Error("a store without a folder keeps something")
	}
	if entries, _ := filepath.Glob(filepath.Join(s.dir, "*", ".tmp-*")); len(entries) != 0 {
		t.Errorf("temporary files left: %v", entries)
	}
}

func TestHashOfSeparatesParts(t *testing.T) {
	if hashOf("ab", "c") == hashOf("a", "bc") {
		t.Error("the parts run together")
	}
}
