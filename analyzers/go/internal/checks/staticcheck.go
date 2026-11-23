package checks

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/gocmd"
	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// staticcheckProblem is one line of `staticcheck -f json`.
type staticcheckProblem struct {
	Code     string `json:"code"`
	Severity string `json:"severity"`
	Location struct {
		File string `json:"file"`
		Line int    `json:"line"`
	} `json:"location"`
	Message string `json:"message"`
}

// staticcheck runs staticcheck on the unit. It exits 1 when it reports
// anything; a package that does not compile is reported as a problem with
// the code "compile".
func (r *Runner) staticcheck(ctx context.Context, tree analyzer.Tree, unit loader.Unit, env []string, path string, c *check) {
	args := append([]string{"-f", "json"}, unit.Patterns...)
	out, err := gocmd.Run(ctx, unit.Dir, unit.Env(env, ""), path, args...)
	if err != nil {
		c.fail(fmt.Sprintf("staticcheck could not run: %v", err))
		return
	}
	problems := 0
	sc := bufio.NewScanner(bytes.NewReader(out.Stdout))
	sc.Buffer(make([]byte, 0, 64<<10), 16<<20)
	for sc.Scan() {
		var p staticcheckProblem
		if json.Unmarshal(sc.Bytes(), &p) != nil {
			continue
		}
		problems++
		switch {
		case p.Code == "compile":
			c.fail(compileFailure(p.Message))
		case p.Severity == "ignored":
		default:
			rel, ok := relFile(tree, unit.Dir, p.Location.File)
			if !ok {
				r.log.Debug("staticcheck finding outside the tree", "file", p.Location.File)
				continue
			}
			var rule *string
			if p.Code != "" {
				rule = ptr(p.Code)
			}
			c.add(protocol.Finding{File: rel, Line: max(p.Location.Line, 1), Message: p.Message, Rule: rule, Severity: staticcheckSeverity(p.Code)})
		}
	}
	if problems == 0 && out.ExitCode != 0 {
		c.fail("staticcheck failed: " + firstTextLine(nil, out.Stderr))
	}
}

// staticcheckSeverity maps a check code to a severity by its category:
// SA (bugs) is an error, U (unused code) a warning, S (simplifications), ST
// (style) and QF (quick fixes) are suggestions. The "severity" staticcheck
// prints says only whether the code matched -fail, not what kind of problem
// it is, so it is not used. An unknown category is a warning.
func staticcheckSeverity(code string) string {
	category := strings.TrimRight(code, "0123456789")
	switch category {
	case "SA":
		return protocol.SeverityError
	case "S", "ST", "QF":
		return protocol.SeverityInfo
	}
	return protocol.SeverityWarning
}

// compileFailure turns staticcheck's "# <package>\n<error>..." message into
// one line.
func compileFailure(message string) string {
	pkg, detail := "", ""
	for line := range strings.SplitSeq(message, "\n") {
		line = strings.TrimSpace(line)
		switch {
		case line == "":
		case strings.HasPrefix(line, "#"):
			if pkg == "" {
				pkg = headerPackage(line)
			}
		case detail == "":
			detail = line
		}
	}
	switch {
	case pkg != "" && detail != "":
		return pkg + " does not compile: " + detail
	case detail != "":
		return "the code does not compile: " + detail
	}
	return "the code does not compile"
}
