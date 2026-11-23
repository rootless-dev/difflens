package dupes

import (
	"cmp"
	"context"
	"slices"
)

// hashBase is the multiplier of the rolling hash, which is computed modulo
// 2^64: the 64-bit FNV prime, odd and large enough to spread token kinds.
const hashBase = 1099511628211

// maxGroup is the most windows a candidate group may hold: a hash shared by
// more windows is boilerplate, and its group is dropped.
const maxGroup = 64

// windowRef is one window: the file it is in and its first token.
type windowRef struct {
	file  int32
	start int32
}

// windows calls fn with the first token and the hash of every run of w
// consecutive tokens of toks within one split, in order.
func windows(toks []tok, w int, fn func(start int32, h uint64)) {
	if w < 1 || w > len(toks) {
		return
	}
	pow := uint64(1) // hashBase^(w-1), the weight of the oldest token
	for range w - 1 {
		pow *= hashBase
	}
	var h uint64
	n := 0 // tokens of the current split in h
	for i, t := range toks {
		if n > 0 && t.Split != toks[i-1].Split {
			h, n = 0, 0
		}
		if n == w {
			h -= value(toks[i-w]) * pow
			n--
		}
		h = h*hashBase + value(t)
		n++
		if n == w {
			fn(int32(i-w+1), h)
		}
	}
}

// value is the term of a token in the hash; never zero.
func value(t tok) uint64 { return uint64(t.Kind) + 1 }

// candidates hashes every window of w tokens that touches the change, then
// every window of every file, and returns the windows sharing the hash of a
// touching window, grouped by hash, sorted by file and first token. Groups
// of one and of more than maxGroup windows are dropped. The groups are
// sorted by their first window, so that the search is deterministic.
//
// After the touching windows, the windows of every file are hashed twice:
// once to count the windows of each hash of the change, once to collect the
// windows of the hashes whose group is kept. Most windows are unique, and
// counting them is far cheaper in memory than collecting them.
func candidates(ctx context.Context, files []*file, w int) [][]windowRef {
	counts := map[uint64]int32{} // windows of each hash of a touching window
	for _, f := range files {
		if ctx.Err() != nil {
			return nil
		}
		if f.touch[len(f.tokens)] == 0 {
			continue
		}
		windows(f.tokens, w, func(start int32, h uint64) {
			if f.touches(start, int32(w)) {
				counts[h] = 0
			}
		})
	}
	if len(counts) == 0 {
		return nil
	}
	for _, f := range files {
		if ctx.Err() != nil {
			return nil
		}
		windows(f.tokens, w, func(_ int32, h uint64) {
			if n, ok := counts[h]; ok && n <= maxGroup {
				counts[h] = n + 1
			}
		})
	}
	byHash := map[uint64][]windowRef{}
	for h, n := range counts {
		if n >= 2 && n <= maxGroup {
			byHash[h] = make([]windowRef, 0, n)
		}
	}
	for i, f := range files {
		if ctx.Err() != nil {
			return nil
		}
		windows(f.tokens, w, func(start int32, h uint64) {
			if group, ok := byHash[h]; ok {
				byHash[h] = append(group, windowRef{file: int32(i), start: start})
			}
		})
	}
	groups := make([][]windowRef, 0, len(byHash))
	for _, group := range byHash {
		groups = append(groups, group)
	}
	// Windows are appended file by file and token by token, so each group
	// is already sorted.
	slices.SortFunc(groups, func(a, b []windowRef) int { return compareWindows(a[0], b[0]) })
	return groups
}

func compareWindows(a, b windowRef) int {
	return cmp.Or(cmp.Compare(a.file, b.file), cmp.Compare(a.start, b.start))
}
