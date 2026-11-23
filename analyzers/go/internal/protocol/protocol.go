// Package protocol holds the language-agnostic output document described by
// analyzers/protocol/analysis.schema.json.
package protocol

import (
	"cmp"
	"slices"
	"strings"
)

// Version is the protocol version this analyzer emits.
const Version = 1

// Document is the single JSON document printed on stdout.
type Document struct {
	Protocol    int          `json:"protocol"`
	Language    string       `json:"language"`
	Analyzer    Analyzer     `json:"analyzer"`
	Functions   []Function   `json:"functions"`
	Calls       []Call       `json:"calls"`
	Externals   []External   `json:"externals"`
	Diagnostics []Diagnostic `json:"diagnostics"`
}

// Analyzer identifies the program that produced the document.
type Analyzer struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// Function is a function, method or closure defined in the source tree.
// Test is true for every function and closure declared in a _test.go file,
// helpers included, not only test entry points.
type Function struct {
	ID            string       `json:"id"`
	Name          string       `json:"name"`
	QualifiedName string       `json:"qualifiedName"`
	Container     string       `json:"container"`
	File          string       `json:"file"`
	StartLine     int          `json:"startLine"`
	EndLine       int          `json:"endLine"`
	Generated     bool         `json:"generated"`
	Test          bool         `json:"test"`
	Complexity    int          `json:"complexity,omitempty"`
	EntryPoints   []EntryPoint `json:"entryPoints,omitempty"`
}

// Entry point kinds.
const (
	EntryHTTPRoute  = "http_route"
	EntryCLICommand = "cli_command"
	EntryMain       = "main"
	EntryTest       = "test"
)

// EntryPoint is a way a function is reached from outside the program.
type EntryPoint struct {
	Kind  string `json:"kind"`
	Label string `json:"label"`
	File  string `json:"file,omitempty"`
	Line  int    `json:"line,omitempty"`
}

// Call kinds and resolutions.
const (
	KindAlways      = "always"
	KindConditional = "conditional"
	KindAsync       = "async"

	ResolutionStatic  = "static"
	ResolutionDynamic = "dynamic"
)

// Relations between a caller and a callee.
const (
	RelationCall         = "call"
	RelationRegistration = "registration"
	RelationReference    = "reference"
)

// Call is one relation from a listed function to a function or an external:
// a call site, a registration with a recognised mechanism (evidence says
// which) or a reference to a function value.
type Call struct {
	Caller     string `json:"caller"`
	Callee     string `json:"callee"`
	Line       int    `json:"line"`
	Kind       string `json:"kind"`
	Resolution string `json:"resolution"`
	Relation   string `json:"relation"`
	Evidence   string `json:"evidence,omitempty"`
}

// External is a callee defined outside the source tree.
type External struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	Container string `json:"container"`
}

// Diagnostic levels.
const (
	LevelWarning = "warning"
	LevelError   = "error"
)

// Diagnostic is a problem that did not stop the analysis.
type Diagnostic struct {
	Level   string `json:"level"`
	Message string `json:"message"`
	File    string `json:"file,omitempty"`
	Line    int    `json:"line,omitempty"`
}

// Normalize removes duplicates and sorts every list so that the same input
// always produces byte-identical output.
func (d *Document) Normalize() {
	d.Functions = dedupeSorted(d.Functions, func(a, b Function) int { return cmp.Compare(a.ID, b.ID) })
	for i := range d.Functions {
		eps := d.Functions[i].EntryPoints
		slices.SortFunc(eps, compareEntryPoints)
		d.Functions[i].EntryPoints = slices.Compact(eps)
	}
	d.Calls = dedupeCalls(d.Calls)
	d.Externals = dedupeSorted(d.Externals, func(a, b External) int { return cmp.Compare(a.ID, b.ID) })
	d.Diagnostics = dedupeSorted(d.Diagnostics, compareDiagnostics)
	if d.Functions == nil {
		d.Functions = []Function{}
	}
	if d.Calls == nil {
		d.Calls = []Call{}
	}
	if d.Externals == nil {
		d.Externals = []External{}
	}
	if d.Diagnostics == nil {
		d.Diagnostics = []Diagnostic{}
	}
}

// dedupeSorted sorts items with compare and keeps the first of each run of
// items that compare equal.
func dedupeSorted[T any](items []T, compare func(a, b T) int) []T {
	slices.SortStableFunc(items, compare)
	return slices.CompactFunc(items, func(a, b T) bool { return compare(a, b) == 0 })
}

// dedupeCalls sorts calls and removes duplicates. Call sites are duplicates
// when every field matches, as their kind and resolution say something of
// their own. Registrations and references are one per (caller, callee, line,
// relation): the evidence of the ones folded together is joined, so that a
// handler registered for GET and POST on one line keeps both routes.
func dedupeCalls(calls []Call) []Call {
	slices.SortStableFunc(calls, compareCalls)
	out := calls[:0]
	for _, c := range calls {
		if n := len(out); n > 0 && sameRelation(out[n-1], c) {
			out[n-1].Evidence = joinEvidence(out[n-1].Evidence, c.Evidence)
			continue
		}
		out = append(out, c)
	}
	return out
}

func sameRelation(a, b Call) bool {
	if a.Relation == RelationCall || b.Relation == RelationCall {
		return compareCalls(a, b) == 0
	}
	return a.Caller == b.Caller && a.Callee == b.Callee && a.Line == b.Line && a.Relation == b.Relation
}

const evidenceSeparator = "; "

func joinEvidence(joined, next string) string {
	if next == "" || slices.Contains(strings.Split(joined, evidenceSeparator), next) {
		return joined
	}
	if joined == "" {
		return next
	}
	return joined + evidenceSeparator + next
}

func compareCalls(a, b Call) int {
	return cmp.Or(
		cmp.Compare(a.Caller, b.Caller),
		cmp.Compare(a.Line, b.Line),
		cmp.Compare(a.Callee, b.Callee),
		cmp.Compare(a.Relation, b.Relation),
		cmp.Compare(a.Kind, b.Kind),
		cmp.Compare(a.Resolution, b.Resolution),
		cmp.Compare(a.Evidence, b.Evidence),
	)
}

func compareEntryPoints(a, b EntryPoint) int {
	return cmp.Or(
		cmp.Compare(a.Kind, b.Kind),
		cmp.Compare(a.Label, b.Label),
		cmp.Compare(a.File, b.File),
		cmp.Compare(a.Line, b.Line),
	)
}

func compareDiagnostics(a, b Diagnostic) int {
	return cmp.Or(
		cmp.Compare(a.File, b.File),
		cmp.Compare(a.Line, b.Line),
		cmp.Compare(a.Level, b.Level),
		cmp.Compare(a.Message, b.Message),
	)
}
