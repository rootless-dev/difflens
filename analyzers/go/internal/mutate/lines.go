package mutate

import (
	"path"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// candidateLines returns, per Go file relative to the tree (test files
// left out), the lines that are candidates (rule 1): added, and spanned by
// a coverage block with a count above zero.
func candidateLines(added map[string][][2]int, blocks []protocol.CoverageBlock) map[string]map[int]bool {
	covered := map[string][][2]int{}
	for _, b := range blocks {
		if b.Count > 0 {
			file := path.Clean(b.File)
			covered[file] = append(covered[file], [2]int{b.StartLine, b.EndLine})
		}
	}
	out := map[string]map[int]bool{}
	for file, ranges := range added {
		file = path.Clean(file)
		spans := covered[file]
		if !strings.HasSuffix(file, ".go") || strings.HasSuffix(file, "_test.go") || len(spans) == 0 {
			continue
		}
		for _, r := range ranges {
			for line := r[0]; line <= r[1]; line++ {
				if spanned(spans, line) {
					if out[file] == nil {
						out[file] = map[int]bool{}
					}
					out[file][line] = true
				}
			}
		}
	}
	return out
}

func spanned(spans [][2]int, line int) bool {
	for _, s := range spans {
		if s[0] <= line && line <= s[1] {
			return true
		}
	}
	return false
}

// lineIndex turns byte offsets of a file into lines and columns counted in
// Unicode code points, both 1-based, as the protocol reports them (token
// positions count columns in bytes).
type lineIndex struct {
	src    []byte
	starts []int // offset of the first byte of each line
}

func newLineIndex(src []byte) *lineIndex {
	starts := []int{0}
	for i, b := range src {
		if b == '\n' {
			starts = append(starts, i+1)
		}
	}
	return &lineIndex{src: src, starts: starts}
}

// position returns the line and column of offset.
func (ix *lineIndex) position(offset int) (line, column int) {
	i := sort.Search(len(ix.starts), func(i int) bool { return ix.starts[i] > offset }) - 1
	return i + 1, utf8.RuneCount(ix.src[ix.starts[i]:offset]) + 1
}
