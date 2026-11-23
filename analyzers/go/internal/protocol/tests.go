package protocol

// Documents of analyzers/protocol/tests.schema.json.

// Prepare is the output of the prepare command.
type Prepare struct {
	Protocol int      `json:"protocol"`
	Writable []string `json:"writable"`
}

// TestRun is the output of the test command.
type TestRun struct {
	Protocol    int           `json:"protocol"`
	Language    string        `json:"language"`
	Runner      Analyzer      `json:"runner"`
	Race        bool          `json:"race"`
	Tests       []Test        `json:"tests"`
	Packages    []TestPackage `json:"packages"`
	Coverage    *Coverage     `json:"coverage"`
	Races       []Race        `json:"races"`
	Diagnostics []string      `json:"diagnostics"`
}

// Test outcomes.
const (
	OutcomePass = "pass"
	OutcomeFail = "fail"
	OutcomeSkip = "skip"
)

// Test is one test or subtest that ran or was skipped. Function, File, Line,
// Output and Command are nil when unknown or not applicable. Package is the
// container of the package it ran in, as Packages lists it. Hierarchy is
// the name of each level of Name, from the top-level test down; Command is
// the shell command that runs the test again on its own from the root of
// the tree.
type Test struct {
	Container  string   `json:"container"`
	Package    string   `json:"package"`
	Name       string   `json:"name"`
	Hierarchy  []string `json:"hierarchy"`
	Function   *string  `json:"function"`
	File       *string  `json:"file"`
	Line       *int     `json:"line"`
	Outcome    string   `json:"outcome"`
	DurationMs int64    `json:"durationMs"`
	Output     *string  `json:"output"`
	Command    *string  `json:"command"`
}

// Package outcomes besides pass and fail.
const (
	OutcomeBuildFailed = "build_failed"
	OutcomeNoTests     = "no_tests"
)

// TestPackage is the overall outcome of one package. Command is the shell
// command that runs the package's tests on their own from the root of the
// tree, or nil.
type TestPackage struct {
	Container  string  `json:"container"`
	Outcome    string  `json:"outcome"`
	DurationMs int64   `json:"durationMs"`
	Output     *string `json:"output"`
	Command    *string `json:"command"`
}

// Coverage is the statement coverage of the project's own code.
type Coverage struct {
	Blocks []CoverageBlock `json:"blocks"`
}

// CoverageBlock is a block of statements and how many times it ran.
type CoverageBlock struct {
	File       string `json:"file"`
	StartLine  int    `json:"startLine"`
	EndLine    int    `json:"endLine"`
	Statements int    `json:"statements"`
	Count      int64  `json:"count"`
}

// Race is one report of the race detector. Test is nil when it was reported
// outside any test.
type Race struct {
	Container string  `json:"container"`
	Test      *string `json:"test"`
	Output    string  `json:"output"`
}

// Selection is what a partial run runs ($defs/selection): packages by the
// container Packages names them with.
type Selection struct {
	Packages []SelectedPackage `json:"packages"`
}

// SelectedPackage is a package to run: all of it when Tests is nil, else
// only those tests.
type SelectedPackage struct {
	Package string         `json:"package"`
	Tests   []SelectedTest `json:"tests"`
}

// SelectedTest is a top-level test to run: all of it when Subtests is nil,
// else only those subtests, named relative to the test ("a" or "a/b"; a
// nested one runs its first-level ancestor).
type SelectedTest struct {
	Name     string   `json:"name"`
	Subtests []string `json:"subtests"`
}

// Progress is what the test command writes to --progress while it runs
// ($defs/progress).
type Progress struct {
	Protocol      int     `json:"protocol"`
	PackagesDone  int     `json:"packagesDone"`
	PackagesTotal int     `json:"packagesTotal"`
	Tests         int     `json:"tests"`
	Failed        int     `json:"failed"`
	Current       *string `json:"current"`
}
