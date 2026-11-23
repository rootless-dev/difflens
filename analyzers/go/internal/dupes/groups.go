package dupes

import (
	"cmp"
	"slices"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// maxDuplicates is the most duplicates a search reports per scope.
const maxDuplicates = 500

// lines is an inclusive range of lines of one file, by its index.
type lines struct {
	file       int32
	start, end int
}

func (l lines) count() int { return l.end - l.start + 1 }

func compareLines(a, b lines) int {
	return cmp.Or(cmp.Compare(a.file, b.file), cmp.Compare(a.start, b.start), cmp.Compare(a.end, b.end))
}

// side is one side of a maximal pair. A side is new when at least half of
// its lines are added, existing otherwise.
type side struct {
	lines
	isNew bool
}

// match is a maximal pair kept by the line and change rules, by its lines.
type match struct {
	a, b   side
	tokens int
}

// classify turns the maximal pairs into matches: a pair with a side that
// spans fewer than minLines lines is dropped, and so is a pair without a new
// side.
func classify(files []*file, pairs []pair, minLines int) []match {
	var matches []match
	for _, p := range pairs {
		a := newSide(files, p.fa, p.a, p.n)
		b := newSide(files, p.fb, p.b, p.n)
		if a.count() < minLines || b.count() < minLines {
			continue
		}
		if !a.isNew && !b.isNew {
			continue
		}
		matches = append(matches, match{a: a, b: b, tokens: int(p.n)})
	}
	return matches
}

func newSide(files []*file, fi, start, n int32) side {
	f := files[fi]
	from, to := f.lines(start, n)
	l := lines{file: fi, start: from, end: to}
	return side{lines: l, isNew: isNew(f.added, l)}
}

// isNew reports whether at least half of the lines of l are added.
func isNew(added lineSet, l lines) bool {
	return 2*added.count(l.start, l.end) >= l.count()
}

// mergeLines sorts ranges by file and start and merges the ranges of a file
// whose lines overlap into their union, until no two overlap. Ranges that
// only touch (one ends on the line before the other starts) stay apart.
func mergeLines(ranges []lines) []lines {
	sorted := slices.Clone(ranges)
	slices.SortFunc(sorted, compareLines)
	var merged []lines
	for _, r := range sorted {
		if n := len(merged); n > 0 && merged[n-1].file == r.file && r.start <= merged[n-1].end {
			merged[n-1].end = max(merged[n-1].end, r.end)
			continue
		}
		merged = append(merged, r)
	}
	return merged
}

// blockOf returns the index of the block, in blocks sorted by mergeLines,
// that holds l.
func blockOf(blocks []lines, l lines) int {
	i, _ := slices.BinarySearchFunc(blocks, l, func(b, l lines) int {
		return cmp.Or(cmp.Compare(b.file, l.file), cmp.Compare(b.start, l.start))
	})
	// i is the first block starting after l, or at its start line.
	if i < len(blocks) && blocks[i].file == l.file && blocks[i].start == l.start {
		return i
	}
	return i - 1
}

// duplicates merges the new sides of the matches into blocks and groups the
// blocks: a block with an existing occurrence copies existing code; blocks
// that are only occurrences of one another are code repeated in the
// change. The result is sorted by block, then by the others of each.
func duplicates(files []*file, matches []match) []protocol.Duplicate {
	var newSides []lines
	for _, m := range matches {
		for _, s := range []side{m.a, m.b} {
			if s.isNew {
				newSides = append(newSides, s.lines)
			}
		}
	}
	blocks := mergeLines(newSides)

	existing := make([][]lines, len(blocks))   // existing occurrences of each block
	existingTokens := make([]int, len(blocks)) // largest pair with an existing side
	type link struct{ a, b, tokens int }
	var links []link // two blocks that are occurrences of one another
	for _, m := range matches {
		switch {
		case m.a.isNew && m.b.isNew:
			a, b := blockOf(blocks, m.a.lines), blockOf(blocks, m.b.lines)
			if a != b {
				links = append(links, link{a, b, m.tokens})
			}
		case m.a.isNew:
			b := blockOf(blocks, m.a.lines)
			existing[b] = append(existing[b], m.b.lines)
			existingTokens[b] = max(existingTokens[b], m.tokens)
		default:
			b := blockOf(blocks, m.b.lines)
			existing[b] = append(existing[b], m.a.lines)
			existingTokens[b] = max(existingTokens[b], m.tokens)
		}
	}

	var out []protocol.Duplicate
	for i, block := range blocks {
		if len(existing[i]) > 0 {
			out = append(out, duplicate(files, protocol.DuplicateCopiesExisting, block, mergeLines(existing[i]), existingTokens[i]))
		}
	}

	// Blocks without an existing occurrence, linked transitively, form one
	// group each.
	parent := make([]int, len(blocks))
	for i := range parent {
		parent[i] = i
	}
	var root func(int) int
	root = func(i int) int {
		if parent[i] != i {
			parent[i] = root(parent[i])
		}
		return parent[i]
	}
	tokens := make([]int, len(blocks)) // largest pair of each group, at its root
	for _, l := range links {
		if len(existing[l.a]) > 0 || len(existing[l.b]) > 0 {
			continue
		}
		ra, rb := root(l.a), root(l.b)
		if ra != rb {
			// The smaller index stays the root: it is the first block.
			ra, rb = min(ra, rb), max(ra, rb)
			parent[rb] = ra
			tokens[ra] = max(tokens[ra], tokens[rb])
		}
		tokens[ra] = max(tokens[ra], l.tokens)
	}
	members := map[int][]lines{}
	for i, block := range blocks {
		if r := root(i); r != i {
			members[r] = append(members[r], block)
		}
	}
	for r, others := range members {
		out = append(out, duplicate(files, protocol.DuplicateRepeatedInChange, blocks[r], others, tokens[r]))
	}

	slices.SortFunc(out, func(a, b protocol.Duplicate) int {
		return cmp.Or(compareRanges(a.Block, b.Block), cmp.Compare(a.Kind, b.Kind))
	})
	return out
}

// duplicate builds the reported group of a block and its other occurrences.
func duplicate(files []*file, kind string, block lines, others []lines, tokens int) protocol.Duplicate {
	d := protocol.Duplicate{Kind: kind, Block: toRange(files, block), Tokens: tokens, Lines: block.count()}
	for _, o := range others {
		d.Others = append(d.Others, toRange(files, o))
	}
	slices.SortFunc(d.Others, compareRanges)
	return d
}

func toRange(files []*file, l lines) protocol.Range {
	return protocol.Range{File: files[l.file].path, StartLine: l.start, EndLine: l.end}
}

func compareRanges(a, b protocol.Range) int {
	return cmp.Or(cmp.Compare(a.File, b.File), cmp.Compare(a.StartLine, b.StartLine), cmp.Compare(a.EndLine, b.EndLine))
}
