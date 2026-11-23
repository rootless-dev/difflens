package checks

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"slices"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/gocmd"
	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// vet runs `go vet -json` on the unit. With -json, vet prints one JSON
// object per package ({} when clean), on stderr up to Go 1.24 and on stdout
// since, and plain text on stderr for packages it could not check: a
// "# <package>" header followed by the errors. Both streams are read.
func (r *Runner) vet(ctx context.Context, tree analyzer.Tree, unit loader.Unit, env []string, c *check, res *Result) {
	args := append([]string{"vet", "-json"}, unit.Patterns...)
	out, err := gocmd.Run(ctx, unit.Dir, unit.Env(env, ""), "go", args...)
	if err != nil {
		c.fail(fmt.Sprintf("go vet could not run: %v", err))
		return
	}
	parsed := parseVet(out.Stdout)
	parsed.merge(parseVet(out.Stderr))
	for _, d := range parsed.diagnostics {
		file, line, ok := position(d.posn)
		if !ok {
			continue
		}
		rel, ok := relFile(tree, unit.Dir, file)
		if !ok {
			r.log.Debug("vet finding outside the tree", "posn", d.posn)
			continue
		}
		// vet has no levels: everything it reports is a likely bug.
		c.add(protocol.Finding{File: rel, Line: max(line, 1), Message: d.message, Rule: ptr(d.analyzer), Severity: protocol.SeverityError})
	}
	for _, f := range parsed.failures {
		c.fail(f)
	}
	res.Diagnostics = append(res.Diagnostics, parsed.analyzerErrors...)
	if out.ExitCode != 0 && len(parsed.failures) == 0 {
		c.fail("go vet failed: " + firstTextLine(parsed.text, out.Stderr))
	}
}

type vetDiagnostic struct {
	analyzer string
	posn     string
	message  string
}

type vetOutput struct {
	diagnostics    []vetDiagnostic
	failures       []string // one per package that could not be checked
	analyzerErrors []string
	text           []string // every line that is not JSON
}

func (out *vetOutput) merge(other vetOutput) {
	out.diagnostics = append(out.diagnostics, other.diagnostics...)
	out.failures = append(out.failures, other.failures...)
	out.analyzerErrors = append(out.analyzerErrors, other.analyzerErrors...)
	out.text = append(out.text, other.text...)
}

// parseVet reads one stream of vet's output: JSON objects, which start with
// a "{" line and end with a "}" line, between plain text lines.
func parseVet(output []byte) vetOutput {
	var out vetOutput
	var object []string // lines of the JSON object being read
	text := vetText{failed: map[string]bool{}}
	sc := bufio.NewScanner(bytes.NewReader(output))
	sc.Buffer(make([]byte, 0, 64<<10), 16<<20)
	for sc.Scan() {
		line := sc.Text()
		switch {
		case object != nil:
			object = append(object, line)
			if line == "}" {
				out.decode(strings.Join(object, "\n"))
				object = nil
			}
		case line == "{":
			object = []string{line}
		case strings.HasPrefix(line, "{") && strings.HasSuffix(line, "}"):
			out.decode(line)
		default:
			text.line(&out, line)
		}
	}
	return out
}

// vetText follows the text sections of vet's output: a "# <package>" header,
// then the errors that kept that package from being checked.
type vetText struct {
	pkg    string // package of the current section
	failed map[string]bool
}

func (t *vetText) line(out *vetOutput, line string) {
	if strings.TrimSpace(line) == "" {
		return
	}
	out.text = append(out.text, line)
	if strings.HasPrefix(line, "# ") {
		if p := headerPackage(line); p != "" {
			t.pkg = p
		}
		return
	}
	if t.pkg != "" && !t.failed[t.pkg] {
		t.failed[t.pkg] = true
		out.failures = append(out.failures, fmt.Sprintf("%s does not compile: %s", t.pkg, strings.TrimPrefix(line, "vet: ")))
	}
}

// decode reads one package object: {"<package>": {"<analyzer>": [...] or {"error": "..."}}}.
func (out *vetOutput) decode(object string) {
	var pkgs map[string]map[string]json.RawMessage
	if err := json.Unmarshal([]byte(object), &pkgs); err != nil {
		out.text = append(out.text, object)
		return
	}
	for _, pkg := range sortedKeys(pkgs) {
		analyzers := pkgs[pkg]
		for _, name := range sortedKeys(analyzers) {
			raw := analyzers[name]
			var diags []struct {
				Posn    string `json:"posn"`
				Message string `json:"message"`
			}
			if err := json.Unmarshal(raw, &diags); err == nil {
				for _, d := range diags {
					out.diagnostics = append(out.diagnostics, vetDiagnostic{analyzer: name, posn: d.Posn, message: d.Message})
				}
				continue
			}
			var failure struct {
				Error string `json:"error"`
			}
			if err := json.Unmarshal(raw, &failure); err == nil && failure.Error != "" {
				out.analyzerErrors = append(out.analyzerErrors, fmt.Sprintf("go vet: analyzer %s failed on %s: %s", name, pkg, failure.Error))
			}
		}
	}
}

// headerPackage returns the package of a "# <package> [<variant>]" header
// line, or "" for a header without one ("# [<variant>]").
func headerPackage(line string) string {
	fields := strings.Fields(strings.TrimPrefix(line, "#"))
	if len(fields) == 0 || strings.HasPrefix(fields[0], "[") {
		return ""
	}
	return fields[0]
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	return keys
}

// firstTextLine returns the first line that says something: not a "#"
// header.
func firstTextLine(text []string, stderr []byte) string {
	for _, line := range text {
		if !strings.HasPrefix(line, "#") && strings.TrimSpace(line) != "" {
			return strings.TrimSpace(line)
		}
	}
	if line := gocmd.FirstLine(stderr); line != "" {
		return line
	}
	return "no output"
}
