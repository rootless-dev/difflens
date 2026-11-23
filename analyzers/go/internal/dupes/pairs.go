package dupes

import "context"

// pair is a maximal pair: n equal tokens from token a of file fa and from
// token b of file fb, (fa, a) before (fb, b). Within one file the two runs
// never overlap.
type pair struct {
	fa, a int32
	fb, b int32
	n     int32
}

// diagonal is the line two equal runs of tokens lie on: their files and the
// distance between their first tokens.
type diagonal struct {
	fa, fb int32
	offset int32
}

// run is the tokens of side a of a diagonal that are equal to side b, from
// start up to (not including) end.
type run struct{ start, end int32 }

// pairFinder turns candidate groups into maximal pairs. Every run it
// extends is remembered on its diagonal, so that the other windows of a
// long clone, which lie on the same run, are not extended again.
type pairFinder struct {
	files   []*file
	w       int32
	covered map[diagonal][]run
	pairs   []pair
}

// findPairs returns the maximal pairs of the candidate groups, each once.
// It stops at the deadline of ctx with the pairs found so far.
func findPairs(ctx context.Context, files []*file, groups [][]windowRef, w int) []pair {
	pf := &pairFinder{files: files, w: int32(w), covered: map[diagonal][]run{}}
	for i, group := range groups {
		if i%256 == 0 && ctx.Err() != nil {
			break
		}
		pf.group(group)
	}
	return pf.pairs
}

// group extends every two windows of a sorted group of which one touches
// the change, whose tokens are really equal and that do not overlap.
func (pf *pairFinder) group(group []windowRef) {
	for i, x := range group {
		fx := pf.files[x.file]
		touchX := fx.touches(x.start, pf.w)
		for _, y := range group[i+1:] {
			fy := pf.files[y.file]
			if !touchX && !fy.touches(y.start, pf.w) {
				continue
			}
			if x.file == y.file && y.start-x.start < pf.w {
				continue // the windows overlap
			}
			d := diagonal{fa: x.file, fb: y.file, offset: y.start - x.start}
			if pf.isCovered(d, x.start) || !equalTokens(fx.tokens[x.start:x.start+pf.w], fy.tokens[y.start:y.start+pf.w]) {
				continue
			}
			r := extend(fx.tokens, fy.tokens, x.start, y.start, pf.w)
			pf.covered[d] = append(pf.covered[d], r)
			pf.emit(d, r)
		}
	}
}

// isCovered reports whether the window from token start of side a of d lies
// on a run already extended.
func (pf *pairFinder) isCovered(d diagonal, start int32) bool {
	for _, r := range pf.covered[d] {
		if r.start <= start && start+pf.w <= r.end {
			return true
		}
	}
	return false
}

// emit records the maximal pairs of run r of diagonal d. Across files, or
// within a file when the two sides do not overlap, that is the whole run.
// Within a file, a run longer than the distance between its sides is code
// repeated back to back: it is cut into consecutive pairs of that distance,
// the last one dropped when shorter than a window, so that the copies are
// paired with each other without the two sides of a pair overlapping.
func (pf *pairFinder) emit(d diagonal, r run) {
	if d.fa != d.fb || r.end-r.start <= d.offset {
		pf.pairs = append(pf.pairs, pair{fa: d.fa, a: r.start, fb: d.fb, b: r.start + d.offset, n: r.end - r.start})
		return
	}
	for start := r.start; start < r.end; start += d.offset {
		n := min(d.offset, r.end-start)
		if n < pf.w {
			break
		}
		pf.pairs = append(pf.pairs, pair{fa: d.fa, a: start, fb: d.fb, b: start + d.offset, n: n})
	}
}

// extend extends the equal windows of w tokens from a[p] and b[q] backward
// and forward while the tokens are equal and stay in the splits of the
// windows, and returns the run on side a.
func extend(a, b []tok, p, q, w int32) run {
	splitA, splitB := a[p].Split, b[q].Split
	start, offset := p, q-p
	for start > 0 && start+offset > 0 &&
		a[start-1].Split == splitA && b[start+offset-1].Split == splitB &&
		a[start-1].Kind == b[start+offset-1].Kind {
		start--
	}
	end := p + w
	for end < int32(len(a)) && end+offset < int32(len(b)) &&
		a[end].Split == splitA && b[end+offset].Split == splitB &&
		a[end].Kind == b[end+offset].Kind {
		end++
	}
	return run{start: start, end: end}
}

func equalTokens(a, b []tok) bool {
	for i := range a {
		if a[i].Kind != b[i].Kind {
			return false
		}
	}
	return true
}
