package mutate

import (
	"cmp"
	"slices"
)

// groupKey identifies the group a mutant is sampled with (rule 4): its
// function, or the top level of its file (scope -1).
type groupKey struct {
	file  string
	scope int // byte offset where the function starts, -1 at top level
}

// sample applies the cap (rule 4). With more mutants than max, it takes
// them round-robin across groups, the functions by file and start, the top
// level of a file placed by its first mutant, each group's mutants in source
// order: the first mutant of every group, then the second, and so on, until
// max are taken. It returns the mutants taken, in their order in ms, and
// whether the cap applied. ms must be sorted by file and offset; the
// sample depends only on them, so the same code gives the same sample.
func sample(ms []*mutant, max int) ([]*mutant, bool) {
	if len(ms) <= max {
		return ms, false
	}
	type group struct {
		key     groupKey
		at      int // where the group sorts in its file
		mutants []*mutant
	}
	var groups []*group
	byKey := map[groupKey]*group{}
	for _, m := range ms {
		g := byKey[m.group]
		if g == nil {
			g = &group{key: m.group, at: m.group.scope}
			if g.at < 0 {
				g.at = m.Start
			}
			byKey[m.group] = g
			groups = append(groups, g)
		}
		g.mutants = append(g.mutants, m)
	}
	slices.SortStableFunc(groups, func(a, b *group) int {
		return cmp.Or(cmp.Compare(a.key.file, b.key.file), cmp.Compare(a.at, b.at))
	})
	taken := map[*mutant]bool{}
	for round := 0; len(taken) < max; round++ {
		for _, g := range groups {
			if round < len(g.mutants) && len(taken) < max {
				taken[g.mutants[round]] = true
			}
		}
	}
	out := make([]*mutant, 0, max)
	for _, m := range ms {
		if taken[m] {
			out = append(out, m)
		}
	}
	return out, true
}
