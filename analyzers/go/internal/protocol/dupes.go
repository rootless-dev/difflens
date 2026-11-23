package protocol

// Documents of analyzers/protocol/dupes.schema.json.

// DupeRun is the output of the dupes command.
type DupeRun struct {
	Protocol    int         `json:"protocol"`
	Language    string      `json:"language"`
	Runner      Analyzer    `json:"runner"`
	MinTokens   int         `json:"minTokens"`
	MinLines    int         `json:"minLines"`
	Duplicates  []Duplicate `json:"duplicates"`
	Diagnostics []string    `json:"diagnostics"`
}

// Duplicate kinds.
const (
	DuplicateCopiesExisting   = "copies_existing"
	DuplicateRepeatedInChange = "repeated_in_change"
)

// Duplicate scopes: the search runs once per scope, so every occurrence of a
// group is production code or every one is test code.
const (
	ScopeProduction = "production"
	ScopeTest       = "test"
)

// Duplicate is one block of added code with its other occurrences. Others is
// never empty.
type Duplicate struct {
	Kind   string  `json:"kind"`
	Scope  string  `json:"scope"`
	Block  Range   `json:"block"`
	Others []Range `json:"others"`
	Tokens int     `json:"tokens"`
	Lines  int     `json:"lines"`
}

// Range is an inclusive range of lines of a file relative to the tree, with
// "/" separators.
type Range struct {
	File      string `json:"file"`
	StartLine int    `json:"startLine"`
	EndLine   int    `json:"endLine"`
}

// Changed is the --changed input of the dupes and mutate commands
// ($defs/changed): the lines the review adds, per file, and the files dupes
// leaves out. Added holds inclusive 1-based [start, end] ranges; they are
// decoded as slices so that a range of the wrong length is seen and rejected
// instead of silently truncated.
type Changed struct {
	Protocol int             `json:"protocol"`
	Exclude  *ChangedExclude `json:"exclude,omitempty"`
	Files    []ChangedFile   `json:"files"`
}

// ChangedExclude is the files dupes leaves out entirely: those under a
// folder named one of Dirs and those whose name ends with one of
// Extensions, both compared ignoring ASCII case.
type ChangedExclude struct {
	Dirs       []string `json:"dirs"`
	Extensions []string `json:"extensions"`
}

// ChangedFile is the added lines of one file.
type ChangedFile struct {
	Path  string  `json:"path"`
	Added [][]int `json:"added"`
}
