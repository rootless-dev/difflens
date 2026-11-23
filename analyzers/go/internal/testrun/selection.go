package testrun

import (
	"regexp"
	"slices"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// invocation is one go test of a unit: the packages it runs, by import path
// or pattern, and the -run pattern that narrows them ("" for all tests).
type invocation struct {
	patterns []string
	run      string
}

// plan returns the go test invocations of a unit and how many package ends
// they will report. Without a selection, one invocation runs every package
// of the unit. With one, the packages of the unit it selects whole share one
// invocation; a package with only some tests gets one for its whole tests
// and one per test with only some subtests, since go test cannot give each
// test its own subtest pattern. listed is the unit's packages as go list
// gave them; found records the containers of the selection seen.
func plan(unitPatterns, listed []string, sel *protocol.Selection, container func(string) string, found map[string]bool) ([]invocation, int) {
	if sel == nil {
		return []invocation{{patterns: unitPatterns}}, len(listed)
	}
	byContainer := map[string]protocol.SelectedPackage{}
	for _, p := range sel.Packages {
		byContainer[p.Package] = p
	}
	var whole []string
	var narrowed []invocation
	for _, path := range listed {
		p, ok := byContainer[container(path)]
		if !ok {
			continue
		}
		found[p.Package] = true
		if p.Tests == nil {
			whole = append(whole, path)
			continue
		}
		for _, run := range runPatterns(p.Tests) {
			narrowed = append(narrowed, invocation{patterns: []string{path}, run: run})
		}
	}
	var invs []invocation
	if len(whole) > 0 {
		invs = append(invs, invocation{patterns: whole})
	}
	invs = append(invs, narrowed...)
	return invs, len(whole) + len(narrowed)
}

// runPatterns returns the -run patterns that run exactly the tests given:
// one for the tests selected whole, '^(A|B)$', then one per test with only
// some subtests, '^T$/^(a|b)$'. A nested subtest runs its first-level
// ancestor. A test selected whole and with subtests runs whole; one with an
// empty list of subtests does not run.
func runPatterns(tests []protocol.SelectedTest) []string {
	var whole []string
	var partial []string
	subtests := map[string][]string{}
	for _, t := range tests {
		if t.Subtests == nil {
			if !slices.Contains(whole, t.Name) {
				whole = append(whole, t.Name)
			}
			continue
		}
		if _, seen := subtests[t.Name]; !seen {
			partial = append(partial, t.Name)
		}
		for _, sub := range t.Subtests {
			first, _, _ := strings.Cut(sub, "/")
			if first != "" && !slices.Contains(subtests[t.Name], first) {
				subtests[t.Name] = append(subtests[t.Name], first)
			}
		}
		if subtests[t.Name] == nil {
			subtests[t.Name] = []string{}
		}
	}
	var patterns []string
	if len(whole) > 0 {
		patterns = append(patterns, alternation(whole))
	}
	for _, name := range partial {
		if slices.Contains(whole, name) || len(subtests[name]) == 0 {
			continue
		}
		patterns = append(patterns, alternation([]string{name})+"/"+alternation(subtests[name]))
	}
	return patterns
}

// alternation matches exactly one of names: '^a$' or '^(a|b)$'.
func alternation(names []string) string {
	quoted := make([]string, len(names))
	for i, name := range names {
		quoted[i] = regexp.QuoteMeta(name)
	}
	if len(quoted) == 1 {
		return "^" + quoted[0] + "$"
	}
	return "^(" + strings.Join(quoted, "|") + ")$"
}

// mergePackages folds the ends of a package reported by several invocations
// into one: the worst outcome (a build failure, then a failure, then a
// pass), the durations summed, the outputs joined.
func mergePackages(packages []protocol.TestPackage) []protocol.TestPackage {
	var out []protocol.TestPackage
	index := map[string]int{}
	for _, p := range packages {
		i, seen := index[p.Container]
		if !seen {
			index[p.Container] = len(out)
			out = append(out, p)
			continue
		}
		merged := &out[i]
		merged.DurationMs += p.DurationMs
		if outcomeRank(p.Outcome) > outcomeRank(merged.Outcome) {
			merged.Outcome = p.Outcome
		}
		switch {
		case p.Output == nil:
		case merged.Output == nil:
			merged.Output = p.Output
		default:
			joined := keepEnd(*merged.Output+*p.Output, maxOutput)
			merged.Output = &joined
		}
	}
	for i := range out {
		// Output is kept only where no failed test explains the outcome.
		if out[i].Outcome == protocol.OutcomePass || out[i].Outcome == protocol.OutcomeNoTests {
			out[i].Output = nil
		}
	}
	return out
}

func outcomeRank(outcome string) int {
	switch outcome {
	case protocol.OutcomeBuildFailed:
		return 3
	case protocol.OutcomeFail:
		return 2
	case protocol.OutcomePass:
		return 1
	default:
		return 0
	}
}
