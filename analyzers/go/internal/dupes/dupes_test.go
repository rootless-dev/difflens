package dupes

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// describe renders tokens as "kind line[-end] split" for readable failures.
func describe(toks []tok) []string {
	var out []string
	for _, t := range toks {
		lines := fmt.Sprint(t.Start)
		if t.End != t.Start {
			lines += fmt.Sprintf("-%d", t.End)
		}
		out = append(out, fmt.Sprintf("%s %s %d", t.Kind, lines, t.Split))
	}
	return out
}

func TestTokenizeNormalizes(t *testing.T) {
	src := "package p\n" +
		"\n" +
		"import \"fmt\"\n" +
		"import (\n" +
		"\tx \"os\"\n" +
		")\n" +
		"\n" +
		"// A comment is dropped.\n" +
		"func f() {\n" +
		"\ts := `a\n" +
		"b` + \"c\" + 'd' /* dropped */\n" +
		"\tn := 1 + 2.5 + 3i + 0x1F\n" +
		"}\n"
	toks, err := tokenize([]byte(src))
	if err != nil {
		t.Fatal(err)
	}
	want := []string{
		"func 9 3", "IDENT 9 3", "( 9 3", ") 9 3", "{ 9 3",
		"IDENT 10 3", ":= 10 3", "STRING 10-11 3", "+ 11 3", "STRING 11 3", "+ 11 3", "STRING 11 3", "; 11 3",
		"IDENT 12 3", ":= 12 3", "INT 12 3", "+ 12 3", "INT 12 3", "+ 12 3", "INT 12 3", "+ 12 3", "INT 12 3", "; 12 3",
		"} 13 3", "; 13 3",
	}
	if got := describe(toks); !slices.Equal(got, want) {
		t.Errorf("tokens:\n got %q\nwant %q", got, want)
	}
}

func TestTokenizeSplitsAtImports(t *testing.T) {
	src := "package p\nvar a = 1\nimport \"fmt\"\nvar b = 2\nimport (\n\t\"os\"\n)\nvar c = 3\n"
	toks, err := tokenize([]byte(src))
	if err != nil {
		t.Fatal(err)
	}
	var splits []int32
	for _, t := range toks {
		if t.Kind.String() == "var" {
			splits = append(splits, t.Split)
		}
	}
	if !slices.Equal(splits, []int32{1, 2, 3}) {
		t.Errorf("splits of the three declarations: %v", splits)
	}
	if len(toks) != 15 {
		t.Errorf("an import was not left out: %q", describe(toks))
	}
}

func TestTokenizeRejectsLexicalErrors(t *testing.T) {
	if _, err := tokenize([]byte("package p\nvar s = \"unterminated\n")); err == nil {
		t.Error("an unterminated string must fail")
	}
}

func TestWindowsStayInSplits(t *testing.T) {
	toks, err := tokenize([]byte("package p\nvar a = b\nimport \"fmt\"\nvar c = d\n"))
	if err != nil {
		t.Fatal(err)
	}
	// Each split is "var ID = ID ;": with w = 5 one window per split, both
	// with the same hash; with w = 6 none, as no window crosses the import.
	var starts []int32
	var hashes []uint64
	windows(toks, 5, func(start int32, h uint64) {
		starts = append(starts, start)
		hashes = append(hashes, h)
	})
	if !slices.Equal(starts, []int32{0, 5}) || hashes[0] != hashes[1] {
		t.Errorf("windows of 5: starts %v hashes %v", starts, hashes)
	}
	n := 0
	windows(toks, 6, func(int32, uint64) { n++ })
	if n != 0 {
		t.Errorf("%d windows of 6 cross the import", n)
	}
}

func TestRollingHashMatchesDirectHash(t *testing.T) {
	toks, err := tokenize([]byte("package p\nfunc f(a, b int) int { return a + b*2 - len(\"x\") }\n"))
	if err != nil {
		t.Fatal(err)
	}
	const w = 4
	windows(toks, w, func(start int32, h uint64) {
		var direct uint64
		for _, t := range toks[start : start+w] {
			direct = direct*hashBase + value(t)
		}
		if h != direct {
			t.Errorf("window %d: rolling %x, direct %x", start, h, direct)
		}
	})
}

func TestNewSides(t *testing.T) {
	added := newLineSet(20, [][2]int{{5, 8}})
	for _, tc := range []struct {
		start, end int
		want       bool
	}{
		{5, 8, true},    // all added
		{3, 10, true},   // 4 of 8: exactly half
		{2, 10, false},  // 4 of 9
		{9, 12, false},  // none
		{8, 8, true},    // one line, added
		{18, 25, false}, // past the end of the file
	} {
		if got := isNew(added, lines{start: tc.start, end: tc.end}); got != tc.want {
			t.Errorf("lines %d-%d: new = %v, want %v", tc.start, tc.end, got, tc.want)
		}
	}
	if isNew(lineSet{}, lines{start: 1, end: 1}) {
		t.Error("a file without added lines has no new side")
	}
}

func TestMergeLines(t *testing.T) {
	got := mergeLines([]lines{
		{file: 1, start: 10, end: 20},
		{file: 0, start: 30, end: 40},
		{file: 1, start: 15, end: 25}, // overlaps the first
		{file: 1, start: 26, end: 30}, // only touches the union: stays apart
		{file: 0, start: 1, end: 5},
		{file: 0, start: 5, end: 5}, // shares line 5
		{file: 1, start: 12, end: 14},
	})
	want := []lines{
		{file: 0, start: 1, end: 5},
		{file: 0, start: 30, end: 40},
		{file: 1, start: 10, end: 25},
		{file: 1, start: 26, end: 30},
	}
	if !slices.Equal(got, want) {
		t.Errorf("merged:\n got %v\nwant %v", got, want)
	}
	for _, l := range []lines{{file: 1, start: 12, end: 14}, {file: 1, start: 10, end: 20}, {file: 1, start: 26, end: 30}, {file: 0, start: 5, end: 5}} {
		if b := got[blockOf(got, l)]; b.file != l.file || b.start > l.start || b.end < l.end {
			t.Errorf("block of %v: %v", l, b)
		}
	}
}

func TestClassifyNeedsBothSidesLongEnough(t *testing.T) {
	files := []*file{
		{path: "long.go", tokens: []tok{{Start: 1, End: 1}, {Start: 8, End: 8}}, added: newLineSet(10, [][2]int{{1, 10}})},
		{path: "short.go", tokens: []tok{{Start: 1, End: 1}, {Start: 3, End: 3}}},
	}
	pairs := []pair{{fa: 0, a: 0, fb: 1, b: 0, n: 2}} // lines 1-8 and 1-3
	if got := classify(files, pairs, 6); len(got) != 0 {
		t.Errorf("a side of 3 lines is under 6: %+v", got)
	}
	got := classify(files, pairs, 3)
	want := []match{{
		a:      side{lines: lines{file: 0, start: 1, end: 8}, isNew: true},
		b:      side{lines: lines{file: 1, start: 1, end: 3}},
		tokens: 2,
	}}
	if !slices.Equal(got, want) {
		t.Errorf("matches: %+v", got)
	}
}

func TestDuplicatesDropsALoneRepeatedBlock(t *testing.T) {
	files := []*file{{path: "a.go"}, {path: "b.go"}, {path: "old.go"}}
	copied := side{lines: lines{file: 0, start: 1, end: 10}, isNew: true}
	repeat := side{lines: lines{file: 1, start: 1, end: 10}, isNew: true}
	old := side{lines: lines{file: 2, start: 1, end: 10}}
	// b.go repeats a.go, which copies old.go: b.go's only new occurrence
	// copies existing code, so it forms no group of its own.
	got := duplicates(files, []match{{a: copied, b: old, tokens: 60}, {a: copied, b: repeat, tokens: 60}})
	want := []protocol.Duplicate{{
		Kind:   protocol.DuplicateCopiesExisting,
		Block:  protocol.Range{File: "a.go", StartLine: 1, EndLine: 10},
		Others: []protocol.Range{{File: "old.go", StartLine: 1, EndLine: 10}},
		Tokens: 60, Lines: 10,
	}}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("duplicates:\n got %+v\nwant %+v", got, want)
	}
}

// writeTree writes files under a temporary directory and returns it.
func writeTree(t *testing.T, files map[string]string) string {
	t.Helper()
	dir := t.TempDir()
	for name, content := range files {
		path := filepath.Join(dir, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

func search(t *testing.T, ctx context.Context, dir string, opts Options) *Result {
	t.Helper()
	res, err := Search(ctx, logging.Discard(), dir, opts)
	if err != nil {
		t.Fatal(err)
	}
	return res
}

// body is a function of 10 lines and 66 tokens, named name.
func body(name string) string {
	return "func " + name + "(items []string, limit int) (int, error) {\n" +
		"\ttotal := 0\n" +
		"\tfor i, item := range items {\n" +
		"\t\tif i >= limit {\n" +
		"\t\t\treturn total, fmt.Errorf(\"too many: %d\", i)\n" +
		"\t\t}\n" +
		"\t\ttotal += len(item) * 2\n" +
		"\t}\n" +
		"\treturn total, nil\n" +
		"}\n"
}

func lineCount(s string) int { return strings.Count(s, "\n") }

func TestSearchSkipsFoldersAndLargeFiles(t *testing.T) {
	big := "package p\n\n" + body("big") + strings.Repeat("// padding to pass the size limit\n", maxFileSize/30)
	dir := writeTree(t, map[string]string{
		"old.go":                "package p\n\n" + body("old"),
		"new.go":                "package p\n\n" + body("fresh"),
		"vendor/v/v.go":         "package v\n\n" + body("vendored"),
		"testdata/t.go":         "package t\n\n" + body("fixture"),
		".hidden/h.go":          "package h\n\n" + body("hidden"),
		"_skip/s.go":            "package s\n\n" + body("skipped"),
		"node_modules/n/n.go":   "package n\n\n" + body("module"),
		"big.go":                big,
		"notgo.txt":             body("text"),
		"sub/deeper/sibling.go": "package deeper\n\nvar unrelated = 1\n",
	})
	res := search(t, t.Context(), dir, Options{MinTokens: 50, MinLines: 6, Added: Added{"new.go": {{1, 12}}}})
	want := []protocol.Duplicate{{
		Kind:   protocol.DuplicateCopiesExisting,
		Scope:  protocol.ScopeProduction,
		Block:  protocol.Range{File: "new.go", StartLine: 3, EndLine: 12},
		Others: []protocol.Range{{File: "old.go", StartLine: 3, EndLine: 12}},
		Tokens: 66, Lines: 10,
	}}
	if fmt.Sprint(res.Duplicates) != fmt.Sprint(want) || len(res.Diagnostics) != 0 {
		t.Errorf("duplicates %+v diagnostics %q", res.Duplicates, res.Diagnostics)
	}
}

func TestSearchNeverPairsTestCodeWithProductionCode(t *testing.T) {
	for name, tc := range map[string]struct {
		existing, added string
		found           int
	}{
		"test copies production":       {existing: "old.go", added: "new_test.go"},
		"production copies test":       {existing: "old_test.go", added: "new.go"},
		"production copies production": {existing: "old.go", added: "new.go", found: 1},
	} {
		t.Run(name, func(t *testing.T) {
			dir := writeTree(t, map[string]string{
				tc.existing: "package p\n\n" + body("old"),
				tc.added:    "package p\n\n" + body("fresh"),
			})
			res := search(t, t.Context(), dir, Options{MinTokens: 50, MinLines: 6, Added: Added{tc.added: {{1, 12}}}})
			if len(res.Duplicates) != tc.found || len(res.Diagnostics) != 0 {
				t.Errorf("duplicates %+v diagnostics %q", res.Duplicates, res.Diagnostics)
			}
		})
	}
}

func TestSearchReportsEachScopeOnItsOwn(t *testing.T) {
	// The test files sort before the production files, but production
	// groups come first.
	dir := writeTree(t, map[string]string{
		"a_test.go":  "package p\n\n" + body("existingTest"),
		"b_test.go":  "package p\n\n" + body("addedTest"),
		"zz/old.go":  "package zz\n\n" + body("calculateShipping"),
		"zz/new.go":  "package zz\n\n" + body("calculateTax"),
		"zz/note.go": "package zz\n\nvar unrelated = 1\n",
	})
	res := search(t, t.Context(), dir, Options{MinTokens: 50, MinLines: 6, Added: Added{
		"b_test.go": {{1, 12}},
		"zz/new.go": {{1, 12}},
	}})
	want := []protocol.Duplicate{
		{
			Kind:   protocol.DuplicateCopiesExisting,
			Scope:  protocol.ScopeProduction,
			Block:  protocol.Range{File: "zz/new.go", StartLine: 3, EndLine: 12},
			Others: []protocol.Range{{File: "zz/old.go", StartLine: 3, EndLine: 12}},
			Tokens: 66, Lines: 10,
		},
		{
			Kind:   protocol.DuplicateCopiesExisting,
			Scope:  protocol.ScopeTest,
			Block:  protocol.Range{File: "b_test.go", StartLine: 3, EndLine: 12},
			Others: []protocol.Range{{File: "a_test.go", StartLine: 3, EndLine: 12}},
			Tokens: 66, Lines: 10,
		},
	}
	if fmt.Sprint(res.Duplicates) != fmt.Sprint(want) || len(res.Diagnostics) != 0 {
		t.Errorf("duplicates:\n got %+v\nwant %+v\ndiagnostics %q", res.Duplicates, want, res.Diagnostics)
	}

	// Without the production clone, only the test group is left.
	res = search(t, t.Context(), dir, Options{MinTokens: 50, MinLines: 6, Added: Added{"b_test.go": {{1, 12}}}})
	if fmt.Sprint(res.Duplicates) != fmt.Sprint(want[1:]) {
		t.Errorf("duplicates: %+v", res.Duplicates)
	}
}

func TestSearchLeavesOutExcludedFiles(t *testing.T) {
	// The tree's own folder is named like an excluded folder: only folders
	// below it count.
	dir := filepath.Join(writeTree(t, map[string]string{}), "locales")
	for name, content := range map[string]string{
		"new.go":             "package p\n\n" + body("fresh"),
		"Locales/en/old.go":  "package en\n\n" + body("english"),
		"sub/messages.PB.go": "package sub\n\n" + body("generatedLike"),
	} {
		path := filepath.Join(dir, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	opts := Options{MinTokens: 50, MinLines: 6, Added: Added{"new.go": {{1, 12}}}}
	others := func(res *Result) []string {
		var files []string
		for _, d := range res.Duplicates {
			for _, o := range d.Others {
				files = append(files, o.File)
			}
		}
		return files
	}

	// Without an exclude, both copies are found.
	if got := others(search(t, t.Context(), dir, opts)); !slices.Equal(got, []string{"Locales/en/old.go", "sub/messages.PB.go"}) {
		t.Errorf("others without exclude: %q", got)
	}

	opts.Exclude = Exclude{Dirs: []string{"", "locales"}, Extensions: []string{"", ".pb.go"}}
	res := search(t, t.Context(), dir, opts)
	if len(res.Duplicates) != 0 || len(res.Diagnostics) != 0 {
		t.Errorf("duplicates %+v diagnostics %q", res.Duplicates, res.Diagnostics)
	}

	// A folder deeper down counts too, but a file name is not a folder: dirs
	// naming the files themselves leave them in.
	opts.Exclude = Exclude{Dirs: []string{"en", "old.go", "messages.PB.go"}}
	if got := others(search(t, t.Context(), dir, opts)); !slices.Equal(got, []string{"sub/messages.PB.go"}) {
		t.Errorf("others with a folder excluded: %q", got)
	}
}

func TestExcludeComparesASCIICaseOnly(t *testing.T) {
	e := Exclude{Dirs: []string{"Locales"}, Extensions: []string{".JSON.go"}}
	for name, want := range map[string]bool{"locales": true, "LOCALES": true, "locale": false, "lócales": false} {
		if got := e.excludesDir(name); got != want {
			t.Errorf("excludesDir(%q) = %v", name, got)
		}
	}
	for name, want := range map[string]bool{"en.json.go": true, "EN.Json.GO": true, "json.go": false, "en.go": false} {
		if got := e.excludesFile(name); got != want {
			t.Errorf("excludesFile(%q) = %v", name, got)
		}
	}
	// "K" (Kelvin sign) folds to "k" in Unicode, not in ASCII.
	if (Exclude{Dirs: []string{"kit"}}).excludesDir("Kit") {
		t.Error("a non-ASCII letter must not fold")
	}
}

func TestSearchCutsCodeRepeatedBackToBack(t *testing.T) {
	stmt := "\tif err := step(ctx, \"name\", 1); err != nil {\n\t\treturn fmt.Errorf(\"step: %w\", err)\n\t}\n"
	src := "package p\n\nfunc run() error {\n" + strings.Repeat(stmt, 9) + "\treturn nil\n}\n"
	dir := writeTree(t, map[string]string{"p.go": src})
	res := search(t, t.Context(), dir, Options{MinTokens: 50, MinLines: 6, Added: Added{"p.go": {{1, lineCount(src)}}}})
	// Each statement is 3 lines and 27 tokens. The run along each distance
	// is cut into back-to-back pairs whose sides do not overlap, but the
	// pairs of the different distances overlap one another, so every new
	// side merges into one block, which is only an occurrence of itself.
	if len(res.Duplicates) != 0 {
		t.Errorf("duplicates: %+v", res.Duplicates)
	}

	// Split into two functions, the halves are two blocks of one group.
	half := strings.Repeat(stmt, 4)
	src = "package p\n\nfunc first() error {\n" + half + "\treturn nil\n}\n\nvar x = 1\n\nfunc second() error {\n" + half + "\treturn nil\n}\n"
	dir = writeTree(t, map[string]string{"p.go": src})
	res = search(t, t.Context(), dir, Options{MinTokens: 50, MinLines: 6, Added: Added{"p.go": {{1, lineCount(src)}}}})
	if len(res.Duplicates) != 1 || res.Duplicates[0].Kind != protocol.DuplicateRepeatedInChange ||
		res.Duplicates[0].Block != (protocol.Range{File: "p.go", StartLine: 3, EndLine: 17}) ||
		len(res.Duplicates[0].Others) != 1 || res.Duplicates[0].Others[0] != (protocol.Range{File: "p.go", StartLine: 21, EndLine: 35}) {
		t.Errorf("duplicates: %+v", res.Duplicates)
	}
}

func TestSearchExtendsALongCloneOnce(t *testing.T) {
	var long strings.Builder
	for i := range 200 {
		fmt.Fprintf(&long, "\tv%d := compute(%d, \"k\") + offset\n", i, i)
	}
	fn := func(name string) string { return "func " + name + "() {\n" + long.String() + "}\n" }
	dir := writeTree(t, map[string]string{"a.go": "package p\n\n" + fn("a"), "b.go": "package p\n\n" + fn("b")})
	root, err := openRoot(dir)
	if err != nil {
		t.Fatal(err)
	}
	res := &Result{}
	files, err := loadFiles(t.Context(), logging.Discard(), root, Added{"b.go": {{1, 300}}}, Exclude{}, res)
	if err != nil {
		t.Fatal(err)
	}
	pairs := findPairs(t.Context(), files, candidates(t.Context(), files, 50), 50)
	// The statements are alike, so every window of one function matches
	// many windows of the other; each distance is one maximal pair.
	seen := map[pair]bool{}
	for _, p := range pairs {
		if seen[p] {
			t.Fatalf("pair %+v found twice", p)
		}
		seen[p] = true
	}
	whole := pair{fa: 0, a: 0, fb: 1, b: 0, n: int32(len(files[0].tokens))}
	if !seen[whole] {
		t.Errorf("the whole function is not a pair: %+v", pairs[:min(len(pairs), 5)])
	}
}

func TestSearchStopsAtTheDeadline(t *testing.T) {
	dir := writeTree(t, map[string]string{"old.go": "package p\n\n" + body("old"), "new.go": "package p\n\n" + body("fresh")})
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	res := search(t, ctx, dir, Options{MinTokens: 50, MinLines: 6, Added: Added{"new.go": {{1, 12}}}})
	if len(res.Duplicates) != 0 || !slices.Equal(res.Diagnostics, []string{timeoutMessage}) {
		t.Errorf("duplicates %+v diagnostics %q", res.Duplicates, res.Diagnostics)
	}
}

func TestSearchReportsFilesItCannotTokenize(t *testing.T) {
	dir := writeTree(t, map[string]string{"bad.go": "package p\nvar s = \"unterminated\n"})
	res := search(t, t.Context(), dir, Options{MinTokens: 50, MinLines: 6})
	if len(res.Diagnostics) != 1 || !strings.HasPrefix(res.Diagnostics[0], "could not tokenize bad.go: ") {
		t.Errorf("diagnostics %q", res.Diagnostics)
	}
}

func TestSearchFailures(t *testing.T) {
	dir := t.TempDir()
	if _, err := Search(t.Context(), logging.Discard(), filepath.Join(dir, "missing"), Options{MinTokens: 1, MinLines: 1}); err == nil {
		t.Error("a missing tree must fail")
	}
	file := filepath.Join(dir, "f.go")
	if err := os.WriteFile(file, []byte("package p\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Search(t.Context(), logging.Discard(), file, Options{MinTokens: 1, MinLines: 1}); err == nil {
		t.Error("a file is not a tree")
	}
	if _, err := Search(t.Context(), logging.Discard(), dir, Options{MinTokens: 0, MinLines: 1}); err == nil {
		t.Error("a minimum of 0 must fail")
	}
}
