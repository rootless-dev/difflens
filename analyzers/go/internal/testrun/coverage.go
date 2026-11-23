package testrun

import (
	"bufio"
	"cmp"
	"fmt"
	"io"
	"slices"
	"strconv"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// profileBlock identifies a block of a coverage profile.
type profileBlock struct {
	file                                 string // as the profile names it
	startLine, startCol, endLine, endCol int
	statements                           int
}

// coverage merges coverage profiles.
//
// With -coverpkg every test binary reports every block of the covered
// packages it links, so the same block appears once per binary. Identical
// blocks are merged by summing their counts, which gives how many times the
// block ran over the whole run (in "set" mode, where counts are 0 or 1, the
// maximum is kept instead).
type coverage struct {
	mode   string
	counts map[profileBlock]int64
}

func newCoverage() *coverage { return &coverage{counts: map[profileBlock]int64{}} }

// add merges one profile in the text format of `go test -coverprofile`.
func (c *coverage) add(r io.Reader) error {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64<<10), 1<<20)
	lineNo := 0
	for sc.Scan() {
		lineNo++
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		if mode, ok := strings.CutPrefix(line, "mode: "); ok {
			c.mode = mode
			continue
		}
		b, count, err := parseProfileLine(line)
		if err != nil {
			return fmt.Errorf("coverage profile line %d: %w", lineNo, err)
		}
		if c.mode == "set" {
			c.counts[b] = max(c.counts[b], count)
		} else {
			c.counts[b] += count
		}
	}
	return sc.Err()
}

// parseProfileLine parses "name.go:line.col,line.col statements count".
func parseProfileLine(line string) (profileBlock, int64, error) {
	var b profileBlock
	rest, countText, ok := cutLast(line, " ")
	if !ok {
		return b, 0, fmt.Errorf("malformed %q", line)
	}
	rest, stmtText, ok := cutLast(rest, " ")
	if !ok {
		return b, 0, fmt.Errorf("malformed %q", line)
	}
	file, span, ok := cutLast(rest, ":")
	if !ok {
		return b, 0, fmt.Errorf("malformed %q", line)
	}
	count, err1 := strconv.ParseInt(countText, 10, 64)
	stmts, err2 := strconv.Atoi(stmtText)
	_, err3 := fmt.Sscanf(span, "%d.%d,%d.%d", &b.startLine, &b.startCol, &b.endLine, &b.endCol)
	if err1 != nil || err2 != nil || err3 != nil || count < 0 || stmts < 0 {
		return b, 0, fmt.Errorf("malformed %q", line)
	}
	b.file, b.statements = file, stmts
	return b, count, nil
}

func cutLast(s, sep string) (before, after string, ok bool) {
	i := strings.LastIndex(s, sep)
	if i < 0 {
		return s, "", false
	}
	return s[:i], s[i+len(sep):], true
}

// blocks returns the merged blocks of the project's own code, files mapped
// by source (which drops test files, generated files and files outside the
// tree), sorted by file and position.
func (c *coverage) blocks(source func(name string) (string, bool)) []protocol.CoverageBlock {
	type located struct {
		profileBlock
		rel   string
		count int64
	}
	var all []located
	for b, count := range c.counts {
		if rel, ok := source(b.file); ok {
			all = append(all, located{b, rel, count})
		}
	}
	slices.SortFunc(all, func(a, b located) int {
		return cmp.Or(
			cmp.Compare(a.rel, b.rel),
			cmp.Compare(a.startLine, b.startLine), cmp.Compare(a.startCol, b.startCol),
			cmp.Compare(a.endLine, b.endLine), cmp.Compare(a.endCol, b.endCol),
			cmp.Compare(a.statements, b.statements),
		)
	})
	out := make([]protocol.CoverageBlock, 0, len(all))
	for _, b := range all {
		out = append(out, protocol.CoverageBlock{
			File: b.rel, StartLine: b.startLine, EndLine: b.endLine, Statements: b.statements, Count: b.count,
		})
	}
	return out
}
