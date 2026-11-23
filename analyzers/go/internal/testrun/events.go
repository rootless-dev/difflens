package testrun

import (
	"bufio"
	"encoding/json"
	"io"
	"math"
	"slices"
	"strings"
	"time"
)

// event is one line of `go test -json`: a cmd/test2json event, or one of the
// build events the go command prints since Go 1.24 ("build-output" and
// "build-fail", keyed by ImportPath).
type event struct {
	Time        time.Time
	Action      string
	Package     string
	Test        string
	Elapsed     float64
	Output      string
	ImportPath  string
	FailedBuild string
}

// testResult is a test or subtest that finished (or never did).
type testResult struct {
	pkg     string
	name    string
	outcome string // pass, fail or skip
	elapsed time.Duration
	output  string // set for failed tests only
}

// packageResult is the end of a package as the stream reported it.
type packageResult struct {
	pkg         string
	action      string // pass, fail, skip, or "" when it never ended
	elapsed     time.Duration
	output      string // package-level output, outside any test
	failedBuild string // the build that failed, as the go command names it
	ranTests    bool
	failedTests bool
}

// raceResult is one race report.
type raceResult struct {
	pkg    string
	test   string // "" outside any test
	output string
}

// collector turns the event stream into results. Events of different
// packages may interleave; events of one package arrive in order.
type collector struct {
	tests    []testResult
	races    []raceResult
	pkgs     map[string]*pkgState
	pkgOrder []string                 // packages in order of first event
	ended    []string                 // packages in order of their end event
	build    map[string]*tail         // build output per ImportPath
	events   int                      // events decoded
	unknown  int                      // lines that were not events
	results  map[string]packageResult // ended packages

	// For the progress of a run: top-level tests ended and failed, the
	// package of the latest event that did not end it, and a hook called
	// after every event (ended tells that a package just ended).
	topTests  int
	topFailed int
	current   string
	notify    func(ended bool)
}

type pkgState struct {
	path    string
	out     *tail
	race    raceScanner
	running []*testState // tests started and not finished, in start order
	ran     bool
	failed  bool
}

type testState struct {
	name  string
	start time.Time
	out   *tail // its output and its subtests'
	race  raceScanner
}

func newCollector() *collector {
	return &collector{pkgs: map[string]*pkgState{}, build: map[string]*tail{}, results: map[string]packageResult{}}
}

// read consumes a whole stream of events.
func (c *collector) read(r io.Reader) error {
	br := bufio.NewReader(r)
	for {
		line, err := br.ReadBytes('\n')
		if len(line) > 0 {
			c.line(line)
		}
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return err
		}
	}
}

func (c *collector) line(line []byte) {
	var ev event
	if len(line) == 0 || line[0] != '{' || json.Unmarshal(line, &ev) != nil {
		c.unknown++
		return
	}
	c.events++
	ended := len(c.ended)
	c.handle(ev)
	if c.notify != nil {
		c.notify(len(c.ended) > ended)
	}
}

func (c *collector) handle(ev event) {
	switch ev.Action {
	case "build-output":
		t := c.build[ev.ImportPath]
		if t == nil {
			t = newTail(maxOutput)
			c.build[ev.ImportPath] = t
		}
		t.WriteString(ev.Output)
		return
	case "build-fail":
		return
	}
	if ev.Package == "" {
		return
	}
	p := c.pkg(ev.Package)
	c.current = ev.Package
	if ev.Test == "" {
		c.packageEvent(p, ev)
	} else {
		c.testEvent(p, ev)
	}
}

func (c *collector) packageEvent(p *pkgState, ev event) {
	switch ev.Action {
	case "output":
		p.out.WriteString(ev.Output)
		c.scanRace(p, &p.race, "", ev.Output)
	case "pass", "fail", "skip":
		c.endPackage(p, ev)
	}
}

func (c *collector) testEvent(p *pkgState, ev event) {
	switch ev.Action {
	case "run":
		p.ran = true
		p.test(ev.Test, ev.Time)
	case "output":
		c.testOutput(p, ev)
	case "pass", "fail", "skip":
		t := p.test(ev.Test, ev.Time)
		c.endTest(p, t, ev.Action, seconds(ev.Elapsed))
	}
}

// testOutput gives a line of a test's output to the test and to its running
// ancestors. Output of a test that is not running (late output of a finished
// test) still goes to its ancestors.
func (c *collector) testOutput(p *pkgState, ev event) {
	if t := p.find(ev.Test); t != nil {
		c.scanRace(p, &t.race, t.name, ev.Output)
	} else {
		c.scanRace(p, &p.race, ev.Test, ev.Output)
	}
	for _, owner := range p.running {
		if owner.name == ev.Test || strings.HasPrefix(ev.Test, owner.name+"/") {
			owner.out.WriteString(ev.Output)
		}
	}
}

func (c *collector) pkg(path string) *pkgState {
	p := c.pkgs[path]
	if p == nil {
		p = &pkgState{path: path, out: newTail(maxOutput)}
		c.pkgs[path] = p
		c.pkgOrder = append(c.pkgOrder, path)
	}
	return p
}

// find returns the running test name, or nil.
func (p *pkgState) find(name string) *testState {
	for _, t := range p.running {
		if t.name == name {
			return t
		}
	}
	return nil
}

// test returns the running test name, starting it if needed.
func (p *pkgState) test(name string, start time.Time) *testState {
	if t := p.find(name); t != nil {
		return t
	}
	t := &testState{name: name, start: start, out: newTail(maxOutput)}
	p.running = append(p.running, t)
	return t
}

func (c *collector) endTest(p *pkgState, t *testState, outcome string, elapsed time.Duration) {
	p.running = slices.DeleteFunc(p.running, func(r *testState) bool { return r == t })
	c.flushRace(p, &t.race, t.name)
	res := testResult{pkg: p.path, name: t.name, outcome: outcome, elapsed: elapsed}
	if outcome == "fail" {
		p.failed = true
		res.output = t.out.String()
	}
	if !strings.Contains(t.name, "/") {
		c.topTests++
		if outcome == "fail" {
			c.topFailed++
		}
	}
	c.tests = append(c.tests, res)
}

// endPackage records the end of a package. Tests still running then never
// finished (the binary panicked or timed out): they are reported as failed,
// deepest first, so that subtests still come before their parents.
func (c *collector) endPackage(p *pkgState, ev event) {
	c.abandonTests(p, ev.Time)
	c.flushRace(p, &p.race, "")
	c.results[p.path] = packageResult{
		pkg:         p.path,
		action:      ev.Action,
		elapsed:     seconds(ev.Elapsed),
		output:      p.out.String(),
		failedBuild: ev.FailedBuild,
		ranTests:    p.ran,
		failedTests: p.failed,
	}
	c.ended = append(c.ended, p.path)
	if c.current == p.path {
		c.current = ""
	}
}

func (c *collector) abandonTests(p *pkgState, end time.Time) {
	running := slices.Clone(p.running)
	slices.SortStableFunc(running, func(a, b *testState) int {
		return strings.Count(b.name, "/") - strings.Count(a.name, "/")
	})
	for _, t := range running {
		var elapsed time.Duration
		if !end.IsZero() && !t.start.IsZero() && end.After(t.start) {
			elapsed = end.Sub(t.start)
		}
		c.endTest(p, t, "fail", elapsed)
	}
}

// finish closes the stream: packages that never ended (the run was stopped)
// get their running tests failed and an end without action.
func (c *collector) finish(at time.Time) {
	for _, path := range c.pkgOrder {
		if _, ok := c.results[path]; ok {
			continue
		}
		p := c.pkgs[path]
		c.abandonTests(p, at)
		c.flushRace(p, &p.race, "")
		c.results[path] = packageResult{pkg: path, output: p.out.String(), ranTests: p.ran, failedTests: p.failed}
		c.ended = append(c.ended, path)
	}
}

func (c *collector) scanRace(p *pkgState, s *raceScanner, test, output string) {
	if report, ok := s.feed(output); ok {
		c.races = append(c.races, raceResult{pkg: p.path, test: test, output: report})
	}
}

func (c *collector) flushRace(p *pkgState, s *raceScanner, test string) {
	if report, ok := s.flush(); ok {
		c.races = append(c.races, raceResult{pkg: p.path, test: test, output: report})
	}
}

func seconds(s float64) time.Duration {
	return time.Duration(math.Round(s * float64(time.Second)))
}

// raceScanner extracts the race detector's reports from a stream of output
// lines. A report starts with "WARNING: DATA RACE" and ends at the next line
// of "=" signs.
type raceScanner struct {
	in  bool
	buf strings.Builder
}

const (
	raceStart = "WARNING: DATA RACE"
	raceRule  = "=================="
)

func (s *raceScanner) feed(line string) (string, bool) {
	switch {
	case !s.in && strings.HasPrefix(line, raceStart):
		s.in = true
		s.buf.Reset()
		s.buf.WriteString(line)
	case s.in && strings.HasPrefix(line, raceRule):
		return s.flush()
	case s.in && s.buf.Len() < maxRaceReport:
		s.buf.WriteString(line)
	}
	return "", false
}

// flush returns a report that was started and not closed.
func (s *raceScanner) flush() (string, bool) {
	if !s.in {
		return "", false
	}
	s.in = false
	return keepStart(s.buf.String(), maxRaceReport), true
}
