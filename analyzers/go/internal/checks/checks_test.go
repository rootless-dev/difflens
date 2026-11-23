package checks

import (
	"testing"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

const vetStderr = `{}
{
	"example.com/lint/report": {
		"printf": [
			{
				"posn": "/src/report/report.go:7:21",
				"end": "/src/report/report.go:7:23",
				"message": "fmt.Printf format %s has arg count of wrong type int"
			}
		]
	}
}
# example.com/testrun/broken
# [example.com/testrun/broken]
vet: broken/broken_test.go:6:16: undefined: missing
{
	"example.com/other": {
		"buildtag": {
			"error": "analysis failed"
		}
	}
}
`

func TestParseVet(t *testing.T) {
	out := parseVet([]byte(vetStderr))
	if len(out.diagnostics) != 1 {
		t.Fatalf("diagnostics: %+v", out.diagnostics)
	}
	d := out.diagnostics[0]
	if d.analyzer != "printf" || d.posn != "/src/report/report.go:7:21" || d.message != "fmt.Printf format %s has arg count of wrong type int" {
		t.Errorf("diagnostic: %+v", d)
	}
	if len(out.failures) != 1 || out.failures[0] != "example.com/testrun/broken does not compile: broken/broken_test.go:6:16: undefined: missing" {
		t.Errorf("failures: %q", out.failures)
	}
	if len(out.analyzerErrors) != 1 {
		t.Errorf("analyzer errors: %q", out.analyzerErrors)
	}
}

func TestPosition(t *testing.T) {
	for posn, want := range map[string]struct {
		file string
		line int
	}{
		"/a/b.go:7:21":      {"/a/b.go", 7},
		"/a/b.go:7":         {"/a/b.go", 7},
		`C:\a\b.go:12:3`:    {`C:\a\b.go`, 12},
		`C:\a\b.go:12`:      {`C:\a\b.go`, 12},
		"rel/b_test.go:1:1": {"rel/b_test.go", 1},
	} {
		file, line, ok := position(posn)
		if !ok || file != want.file || line != want.line {
			t.Errorf("%s: got %s:%d %v", posn, file, line, ok)
		}
	}
	if _, _, ok := position("no position"); ok {
		t.Error("a message without position parsed")
	}
}

func TestFirstTextLine(t *testing.T) {
	if got := firstTextLine([]string{"# example.com/p", "  ", "go: no Go files"}, nil); got != "go: no Go files" {
		t.Errorf("got %q", got)
	}
	if got := firstTextLine(nil, []byte("\n  vet: crashed\nmore\n")); got != "vet: crashed" {
		t.Errorf("got %q", got)
	}
	if got := firstTextLine(nil, nil); got != "no output" {
		t.Errorf("got %q", got)
	}
}

func TestStaticcheckSeverity(t *testing.T) {
	for code, want := range map[string]string{
		"SA4006": protocol.SeverityError,
		"SA1019": protocol.SeverityError,
		"U1000":  protocol.SeverityWarning,
		"S1000":  protocol.SeverityInfo,
		"ST1003": protocol.SeverityInfo,
		"QF1001": protocol.SeverityInfo,
		"XX1":    protocol.SeverityWarning,
		"":       protocol.SeverityWarning,
	} {
		if got := staticcheckSeverity(code); got != want {
			t.Errorf("%q: got %s, want %s", code, got, want)
		}
	}
}

func TestCompileFailure(t *testing.T) {
	got := compileFailure("# example.com/broken/bad\nbad/bad.go:6:14: cannot use x")
	if got != "example.com/broken/bad does not compile: bad/bad.go:6:14: cannot use x" {
		t.Errorf("got %q", got)
	}
	if got := compileFailure("# \n"); got != "the code does not compile" {
		t.Errorf("got %q", got)
	}
}
