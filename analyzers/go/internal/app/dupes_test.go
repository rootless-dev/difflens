package app

import (
	"bytes"
	"encoding/json"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

const dupesSchemaPath = "../../../protocol/dupes.schema.json"

func runDupes(t *testing.T, dir string, extra ...string) (*protocol.DupeRun, []byte) {
	t.Helper()
	args := append([]string{"dupes", "--dir", dir, "--changed", filepath.Join(dir, "changed.json")}, extra...)
	out, stderr, code := runCLI(t, args...)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	var doc protocol.DupeRun
	if err := json.Unmarshal(out, &doc); err != nil {
		t.Fatalf("decode: %v", err)
	}
	return &doc, out
}

func TestDupesCommand(t *testing.T) {
	dir := fixtureCopy(t, "dupes")
	before := hashTree(t, dir)
	doc, out := runDupes(t, dir)
	requireSchema(t, dupesSchemaPath, out)
	if doc.Protocol != 1 || doc.Language != "go" || doc.Runner.Version != Version || doc.MinTokens != 50 || doc.MinLines != 6 {
		t.Errorf("header: %+v", doc)
	}
	// The helper copied with other names and literals, and a function
	// repeated three times in the change. Not reported: the 4-line copy of
	// clamp (too short), the two alike functions of legacy.go (no added
	// line), the import blocks (left out) and the copy in a generated file.
	want := []protocol.Duplicate{
		{
			Kind:   protocol.DuplicateCopiesExisting,
			Scope:  protocol.ScopeProduction,
			Block:  protocol.Range{File: "added.go", StartLine: 27, EndLine: 40},
			Others: []protocol.Range{{File: "helper.go", StartLine: 31, EndLine: 44}},
			Tokens: 94, Lines: 14,
		},
		{
			Kind:  protocol.DuplicateRepeatedInChange,
			Scope: protocol.ScopeProduction,
			Block: protocol.Range{File: "repeated.go", StartLine: 3, EndLine: 15},
			Others: []protocol.Range{
				{File: "repeated.go", StartLine: 17, EndLine: 29},
				{File: "repeated.go", StartLine: 31, EndLine: 43},
			},
			Tokens: 92, Lines: 13,
		},
	}
	if !slices.EqualFunc(doc.Duplicates, want, equalDuplicates) {
		t.Errorf("duplicates:\n got %+v\nwant %+v", doc.Duplicates, want)
	}
	if len(doc.Diagnostics) != 0 {
		t.Errorf("diagnostics: %q", doc.Diagnostics)
	}

	again, _, _ := runCLI(t, "dupes", "--dir", dir, "--changed", filepath.Join(dir, "changed.json"))
	if !bytes.Equal(again, out) {
		t.Error("two runs printed different documents")
	}
	if hashTree(t, dir) != before {
		t.Error("dupes wrote inside the tree")
	}
}

func equalDuplicates(a, b protocol.Duplicate) bool {
	return a.Kind == b.Kind && a.Scope == b.Scope && a.Block == b.Block && slices.Equal(a.Others, b.Others) &&
		a.Tokens == b.Tokens && a.Lines == b.Lines
}

func TestDupesThresholds(t *testing.T) {
	dir := fixtureCopy(t, "dupes")
	doc, out := runDupes(t, dir, "--min-tokens", "30", "--min-lines", "3")
	requireSchema(t, dupesSchemaPath, out)
	if doc.MinTokens != 30 || doc.MinLines != 3 {
		t.Errorf("minimums: %d %d", doc.MinTokens, doc.MinLines)
	}
	// The copy of clamp is now long enough.
	clamp := protocol.Duplicate{
		Kind:   protocol.DuplicateCopiesExisting,
		Scope:  protocol.ScopeProduction,
		Block:  protocol.Range{File: "added.go", StartLine: 53, EndLine: 59},
		Others: []protocol.Range{{File: "helper.go", StartLine: 49, EndLine: 55}},
		Tokens: 39, Lines: 7,
	}
	if len(doc.Duplicates) != 3 || !equalDuplicates(doc.Duplicates[1], clamp) {
		t.Errorf("duplicates: %+v", doc.Duplicates)
	}

	doc, _ = runDupes(t, dir, "--min-tokens", "100")
	if len(doc.Duplicates) != 0 {
		t.Errorf("no copy is 100 tokens long: %+v", doc.Duplicates)
	}
	doc, _ = runDupes(t, dir, "--min-lines", "14")
	if len(doc.Duplicates) != 1 || doc.Duplicates[0].Block.File != "added.go" {
		t.Errorf("only the 14-line copy remains: %+v", doc.Duplicates)
	}
}

func TestDupesUsageErrors(t *testing.T) {
	dir := t.TempDir()
	changed := func(content string) string {
		path := filepath.Join(t.TempDir(), "changed.json")
		writeFile(t, path, content)
		return path
	}
	valid := changed(`{"protocol": 1, "files": []}`)
	for name, args := range map[string][]string{
		"no --changed":           {"--dir", dir},
		"no --dir":               {"--changed", valid},
		"missing changed file":   {"--dir", dir, "--changed", filepath.Join(dir, "missing.json")},
		"--min-tokens 0":         {"--dir", dir, "--changed", valid, "--min-tokens", "0"},
		"--min-lines -1":         {"--dir", dir, "--changed", valid, "--min-lines", "-1"},
		"--min-lines not number": {"--dir", dir, "--changed", valid, "--min-lines", "six"},
		"positional argument":    {"--dir", dir, "--changed", valid, "extra"},
		"not JSON":               {"--dir", dir, "--changed", changed(`files`)},
		"unknown field":          {"--dir", dir, "--changed", changed(`{"protocol": 1, "files": [], "extra": true}`)},
		"unknown file field":     {"--dir", dir, "--changed", changed(`{"protocol": 1, "files": [{"path": "a.go", "added": [], "removed": []}]}`)},
		"protocol 2":             {"--dir", dir, "--changed", changed(`{"protocol": 2, "files": []}`)},
		"no protocol":            {"--dir", dir, "--changed", changed(`{"files": []}`)},
		"no files":               {"--dir", dir, "--changed", changed(`{"protocol": 1}`)},
		"no added":               {"--dir", dir, "--changed", changed(`{"protocol": 1, "files": [{"path": "a.go"}]}`)},
		"range of one line":      {"--dir", dir, "--changed", changed(`{"protocol": 1, "files": [{"path": "a.go", "added": [[3]]}]}`)},
		"range of three lines":   {"--dir", dir, "--changed", changed(`{"protocol": 1, "files": [{"path": "a.go", "added": [[1, 2, 3]]}]}`)},
		"line 0":                 {"--dir", dir, "--changed", changed(`{"protocol": 1, "files": [{"path": "a.go", "added": [[0, 2]]}]}`)},
		"reversed range":         {"--dir", dir, "--changed", changed(`{"protocol": 1, "files": [{"path": "a.go", "added": [[5, 4]]}]}`)},
	} {
		t.Run(name, func(t *testing.T) {
			out, stderr, code := runCLI(t, append([]string{"dupes"}, args...)...)
			if code != exitUsage || len(out) != 0 || stderr == "" {
				t.Errorf("exit %d, stdout %q, stderr %q", code, out, stderr)
			}
		})
	}
}

func TestDupesFailures(t *testing.T) {
	changed := filepath.Join(t.TempDir(), "changed.json")
	writeFile(t, changed, `{"protocol": 1, "files": []}`)
	if _, _, code := runCLI(t, "dupes", "--dir", changed, "--changed", changed); code != exitFailed {
		t.Errorf("a file as the tree: exit %d", code)
	}
	if _, _, code := runCLI(t, "dupes", "--dir", filepath.Join(t.TempDir(), "missing"), "--changed", changed); code != exitFailed {
		t.Errorf("a missing tree: exit %d", code)
	}
}

func TestDupesWithoutAddedLines(t *testing.T) {
	dir := fixtureCopy(t, "dupes")
	writeFile(t, filepath.Join(dir, "changed.json"), `{"protocol": 1, "files": [{"path": "./helper.go", "added": []}]}`)
	doc, out := runDupes(t, dir)
	requireSchema(t, dupesSchemaPath, out)
	if len(doc.Duplicates) != 0 || len(doc.Diagnostics) != 0 || !strings.HasSuffix(string(out), "\"duplicates\":[],\"diagnostics\":[]}\n") {
		t.Errorf("document: %s", out)
	}
}

// cloneSource is a Go file holding one 10-line function named name, long
// enough to be a duplicate at the default minimums.
func cloneSource(pkg, name string) string {
	return "package " + pkg + "\n\nfunc " + name + "(items []string, limit int) (int, error) {\n" +
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

func TestDupesScopesAndExclude(t *testing.T) {
	dir := t.TempDir()
	for name, content := range map[string]string{
		"shipping.go":            cloneSource("acme", "calculateShipping"),
		"tax.go":                 cloneSource("acme", "calculateTax"),
		"shipping_test.go":       cloneSource("acme", "shippingCases"),
		"tax_test.go":            cloneSource("acme", "taxCases"),
		"Locales/pt/messages.go": cloneSource("pt", "messages"),
		"catalog.I18N.go":        cloneSource("acme", "catalog"),
	} {
		writeFile(t, filepath.Join(dir, filepath.FromSlash(name)), content)
	}
	writeFile(t, filepath.Join(dir, "changed.json"), `{
		"protocol": 1,
		"exclude": {"dirs": ["locales"], "extensions": [".i18n.go"]},
		"files": [
			{"path": "tax.go", "added": [[1, 12]]},
			{"path": "tax_test.go", "added": [[1, 12]]},
			{"path": "Locales/pt/messages.go", "added": [[1, 12]]},
			{"path": "catalog.I18N.go", "added": [[1, 12]]}
		]
	}`)
	doc, out := runDupes(t, dir)
	requireSchema(t, dupesSchemaPath, out)
	// The production clone comes first, then the test clone; the test file
	// is never paired with production code, and the excluded files are
	// neither blocks nor others.
	want := []protocol.Duplicate{
		{
			Kind:   protocol.DuplicateCopiesExisting,
			Scope:  protocol.ScopeProduction,
			Block:  protocol.Range{File: "tax.go", StartLine: 3, EndLine: 12},
			Others: []protocol.Range{{File: "shipping.go", StartLine: 3, EndLine: 12}},
			Tokens: 66, Lines: 10,
		},
		{
			Kind:   protocol.DuplicateCopiesExisting,
			Scope:  protocol.ScopeTest,
			Block:  protocol.Range{File: "tax_test.go", StartLine: 3, EndLine: 12},
			Others: []protocol.Range{{File: "shipping_test.go", StartLine: 3, EndLine: 12}},
			Tokens: 66, Lines: 10,
		},
	}
	if !slices.EqualFunc(doc.Duplicates, want, equalDuplicates) || len(doc.Diagnostics) != 0 {
		t.Errorf("duplicates:\n got %+v\nwant %+v\ndiagnostics %q", doc.Duplicates, want, doc.Diagnostics)
	}
	if !strings.Contains(string(out), `"scope":"production"`) || !strings.Contains(string(out), `"scope":"test"`) {
		t.Errorf("document: %s", out)
	}
}

func TestDupesRejectsAMalformedExclude(t *testing.T) {
	dir := t.TempDir()
	for name, content := range map[string]string{
		"unknown exclude field": `{"protocol": 1, "exclude": {"dirs": [], "extensions": [], "files": []}, "files": []}`,
		"dirs not a list":       `{"protocol": 1, "exclude": {"dirs": "locales", "extensions": []}, "files": []}`,
	} {
		t.Run(name, func(t *testing.T) {
			changed := filepath.Join(t.TempDir(), "changed.json")
			writeFile(t, changed, content)
			out, stderr, code := runCLI(t, "dupes", "--dir", dir, "--changed", changed)
			if code != exitUsage || len(out) != 0 || stderr == "" {
				t.Errorf("exit %d, stdout %q, stderr %q", code, out, stderr)
			}
		})
	}
}

func TestUsageListsDupes(t *testing.T) {
	_, stderr, code := runCLI(t, "help")
	if code != exitUsage || !strings.Contains(stderr, " dupes --dir <source tree> --changed <file>") {
		t.Errorf("exit %d, usage %q", code, stderr)
	}
}
