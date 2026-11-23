package protocol

// Documents of analyzers/protocol/mutation.schema.json.

// MutationRun is the output of the mutate command.
type MutationRun struct {
	Protocol    int             `json:"protocol"`
	Language    string          `json:"language"`
	Runner      Analyzer        `json:"runner"`
	Engine      Analyzer        `json:"engine"`
	Mutants     []Mutant        `json:"mutants"`
	Summary     MutationSummary `json:"summary"`
	Diagnostics []string        `json:"diagnostics"`
}

// Mutation operators (the families of rule 2).
const (
	OperatorRelationalBoundary = "relational_boundary"
	OperatorRelationalNegation = "relational_negation"
	OperatorArithmetic         = "arithmetic"
	OperatorLogical            = "logical"
	OperatorNegationRemoval    = "negation_removal"
	OperatorReturnValue        = "return_value"
	OperatorCallRemoval        = "call_removal"
	OperatorIncrement          = "increment"
)

// Mutant statuses.
const (
	MutantKilled     = "killed"
	MutantSurvived   = "survived"
	MutantTimeout    = "timeout"
	MutantError      = "error"
	MutantNoCoverage = "no_coverage"
)

// Mutant is one small change of a changed line and what its tests said
// about it. Lines and columns are 1-based, columns in Unicode code points,
// EndColumn exclusive. Function is nil at top level; KilledBy is nil unless
// the mutant was killed by a test the engine could name.
type Mutant struct {
	ID          string  `json:"id"`
	File        string  `json:"file"`
	Line        int     `json:"line"`
	Column      int     `json:"column"`
	EndLine     int     `json:"endLine"`
	EndColumn   int     `json:"endColumn"`
	Original    string  `json:"original"`
	Mutated     string  `json:"mutated"`
	Operator    string  `json:"operator"`
	Description string  `json:"description"`
	Function    *string `json:"function"`
	Status      string  `json:"status"`
	KilledBy    *string `json:"killedBy"`
	DurationMs  int64   `json:"durationMs"`
	Cached      bool    `json:"cached"`
}

// MutationSummary counts the mutants of a run. Candidates counts them
// before the cap, the others those reported.
type MutationSummary struct {
	Candidates int   `json:"candidates"`
	Sampled    bool  `json:"sampled"`
	MaxMutants int   `json:"maxMutants"`
	Killed     int   `json:"killed"`
	Survived   int   `json:"survived"`
	Timeout    int   `json:"timeout"`
	Error      int   `json:"error"`
	NoCoverage int   `json:"noCoverage"`
	Cached     int   `json:"cached"`
	DurationMs int64 `json:"durationMs"`
}

// MutationCoverage is the --coverage input of the mutate command
// ($defs/coverage): the blocks of the last test run with coverage.
type MutationCoverage struct {
	Protocol int             `json:"protocol"`
	Blocks   []CoverageBlock `json:"blocks"`
}

// Phases of a mutation run.
const (
	PhaseBaseline = "baseline"
	PhaseMutating = "mutating"
)

// MutationProgress is what the mutate command writes to --progress while it
// runs ($defs/progress). Error counts errors and no_coverage together.
type MutationProgress struct {
	Protocol int     `json:"protocol"`
	Phase    string  `json:"phase"`
	Total    int     `json:"total"`
	Done     int     `json:"done"`
	Killed   int     `json:"killed"`
	Survived int     `json:"survived"`
	Timeout  int     `json:"timeout"`
	Error    int     `json:"error"`
	Current  *string `json:"current"`
}
