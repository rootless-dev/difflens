package protocol

// Document of analyzers/protocol/checks.schema.json.

// CheckRun is the output of the check command.
type CheckRun struct {
	Protocol    int      `json:"protocol"`
	Language    string   `json:"language"`
	Runner      Analyzer `json:"runner"`
	Checks      []Check  `json:"checks"`
	Diagnostics []string `json:"diagnostics"`
}

// Checker statuses.
const (
	CheckClean       = "clean"
	CheckFindings    = "findings"
	CheckUnavailable = "unavailable"
	CheckFailed      = "failed"
)

// Check is the result of one checker. Message is set for unavailable and
// failed only.
type Check struct {
	Tool     string    `json:"tool"`
	Status   string    `json:"status"`
	Message  *string   `json:"message"`
	Findings []Finding `json:"findings"`
}

// Finding severities.
const (
	SeverityError   = "error"
	SeverityWarning = "warning"
	SeverityInfo    = "info"
)

// Finding is one problem a checker reported. Rule is nil when the checker
// gives no code. Severity is the checker's own level, mapped to one of the
// Severity constants.
type Finding struct {
	File     string  `json:"file"`
	Line     int     `json:"line"`
	Message  string  `json:"message"`
	Rule     *string `json:"rule"`
	Severity string  `json:"severity"`
}
