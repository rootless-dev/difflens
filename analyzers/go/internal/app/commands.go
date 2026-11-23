package app

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path"
	"time"

	"github.com/rootless-dev/difflens/analyzers/go/internal/dupes"
	"github.com/rootless-dev/difflens/analyzers/go/internal/mutate"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
	"github.com/rootless-dev/difflens/analyzers/go/internal/testrun"
)

// Default timeouts of the commands that run the toolchain.
const (
	DefaultPrepareTimeout = 10 * time.Minute
	DefaultTestTimeout    = 10 * time.Minute
	DefaultCheckTimeout   = 5 * time.Minute
)

// runPrepare downloads what the tests need and prints the directories the
// test command writes outside the tree (tests.schema.json, $defs/prepare):
// the build cache, the user's or the one in --cache-dir.
func (a *App) runPrepare(args []string) int {
	flags := a.newFlags("prepare", DefaultPrepareTimeout)
	flags.withCacheDir()
	flags.withTools()
	if !a.parse(flags, args) {
		return exitUsage
	}
	ctx, cancel := context.WithTimeout(context.Background(), *flags.timeout)
	defer cancel()
	start := time.Now()
	writable, err := a.tests.Prepare(ctx, *flags.dir, flags.cacheDirValue())
	if err != nil {
		a.log.Error("prepare failed", "dir", *flags.dir, "error", err.Error())
		return exitFailed
	}
	if code := a.write(protocol.Prepare{Protocol: protocol.Version, Writable: writable}); code != exitOK {
		return code
	}
	a.log.Info("prepare done", "elapsed", time.Since(start).Round(time.Millisecond).String())
	return exitOK
}

// runTest runs the tests once and prints tests.schema.json. Failing tests
// are results, not a failure of the command. Every test carries its
// hierarchy (its name split at "/") and, like every package, the go test
// command that runs it again on its own from the root of the tree.
func (a *App) runTest(args []string) int {
	flags := a.newFlags("test", DefaultTestTimeout)
	flags.withCacheDir()
	offline := flags.Bool("offline", false, "no network: never download modules")
	noRace := flags.Bool("no-race", false, "run without the race detector")
	noCover := flags.Bool("no-cover", false, "run without measuring coverage")
	selectionFile := flags.String("selection", "", "JSON file naming the packages and tests to run (tests.schema.json, $defs/selection)")
	progressFile := flags.String("progress", "", "file to keep up to date with the progress of the run")
	if !a.parse(flags, args) {
		return exitUsage
	}
	selection, err := readSelection(*selectionFile)
	if err != nil {
		a.log.Error("read selection", "file", *selectionFile, "error", err.Error())
		fmt.Fprintf(a.stderr, "test: cannot read the selection: %v\n", err)
		return exitUsage
	}
	start := time.Now()
	// The runner enforces the timeout itself, so that it can still report
	// what ran when the time is up.
	res, err := a.tests.Run(context.Background(), *flags.dir, testrun.Options{
		Timeout: *flags.timeout, Offline: *offline, Race: !*noRace, Cover: !*noCover,
		Selection: selection, Progress: *progressFile, CacheDir: flags.cacheDirValue(),
	})
	if err != nil {
		a.log.Error("test run failed", "dir", *flags.dir, "error", err.Error())
		return exitFailed
	}
	doc := protocol.TestRun{
		Protocol:    protocol.Version,
		Language:    "go",
		Runner:      protocol.Analyzer{Name: Name, Version: Version},
		Race:        res.Race,
		Tests:       res.Tests,
		Packages:    res.Packages,
		Coverage:    res.Coverage,
		Races:       res.Races,
		Diagnostics: res.Diagnostics,
	}
	if code := a.write(doc); code != exitOK {
		return code
	}
	a.log.Info("test run done", "elapsed", time.Since(start).Round(time.Millisecond).String(),
		"tests", len(res.Tests), "packages", len(res.Packages), "races", len(res.Races))
	return exitOK
}

// readSelection reads a selection file; "" selects everything (nil).
func readSelection(file string) (*protocol.Selection, error) {
	if file == "" {
		return nil, nil
	}
	data, err := os.ReadFile(file)
	if err != nil {
		return nil, err
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	var selection protocol.Selection
	if err := dec.Decode(&selection); err != nil {
		return nil, err
	}
	if selection.Packages == nil {
		return nil, errors.New("packages is missing")
	}
	return &selection, nil
}

// runCheck runs the static checkers and prints checks.schema.json, every
// finding with its severity: go vet's are errors, staticcheck's by category.
func (a *App) runCheck(args []string) int {
	flags := a.newFlags("check", DefaultCheckTimeout)
	flags.withCacheDir()
	if !a.parse(flags, args) {
		return exitUsage
	}
	ctx, cancel := context.WithTimeout(context.Background(), *flags.timeout)
	defer cancel()
	start := time.Now()
	res, err := a.checks.Run(ctx, *flags.dir, flags.cacheDirValue())
	if err != nil {
		a.log.Error("checks failed", "dir", *flags.dir, "error", err.Error())
		return exitFailed
	}
	doc := protocol.CheckRun{
		Protocol:    protocol.Version,
		Language:    "go",
		Runner:      protocol.Analyzer{Name: Name, Version: Version},
		Checks:      res.Checks,
		Diagnostics: res.Diagnostics,
	}
	if code := a.write(doc); code != exitOK {
		return code
	}
	a.log.Info("checks done", "elapsed", time.Since(start).Round(time.Millisecond).String())
	return exitOK
}

// runDupes looks for the duplicated code the change introduces and prints
// dupes.schema.json. A search that runs out of time reports what it found.
func (a *App) runDupes(args []string) int {
	flags := a.newFlags("dupes", DefaultTimeout)
	changedFile := flags.String("changed", "", "JSON file of the lines the change adds (dupes.schema.json, $defs/changed; required)")
	minTokens := flags.Int("min-tokens", dupes.DefaultMinTokens, "fewest normalized tokens of a duplicate")
	minLines := flags.Int("min-lines", dupes.DefaultMinLines, "fewest lines of a duplicate")
	if !a.parse(flags, args) {
		return exitUsage
	}
	if *minTokens < 1 || *minLines < 1 {
		fmt.Fprintln(a.stderr, "dupes: --min-tokens and --min-lines must be at least 1")
		return exitUsage
	}
	added, exclude, err := readChanged(*changedFile)
	if err != nil {
		a.log.Error("read changed lines", "file", *changedFile, "error", err.Error())
		fmt.Fprintf(a.stderr, "dupes: cannot read the changed lines: %v\n", err)
		return exitUsage
	}
	ctx, cancel := context.WithTimeout(context.Background(), *flags.timeout)
	defer cancel()
	start := time.Now()
	res, err := dupes.Search(ctx, a.log, *flags.dir, dupes.Options{
		MinTokens: *minTokens, MinLines: *minLines, Added: added, Exclude: exclude,
	})
	if err != nil {
		a.log.Error("duplicate search failed", "dir", *flags.dir, "error", err.Error())
		return exitFailed
	}
	doc := protocol.DupeRun{
		Protocol:    protocol.Version,
		Language:    "go",
		Runner:      protocol.Analyzer{Name: Name, Version: Version},
		MinTokens:   *minTokens,
		MinLines:    *minLines,
		Duplicates:  res.Duplicates,
		Diagnostics: res.Diagnostics,
	}
	if code := a.write(doc); code != exitOK {
		return code
	}
	a.log.Info("duplicate search done", "elapsed", time.Since(start).Round(time.Millisecond).String(),
		"duplicates", len(res.Duplicates), "diagnostics", len(res.Diagnostics))
	return exitOK
}

// runMutate tests the added lines the tests execute with mutants and prints
// mutation.schema.json. Surviving mutants are results, not a failure of the
// command; a run that runs out of time reports the mutants it tested.
func (a *App) runMutate(args []string) int {
	flags := a.newFlags("mutate", DefaultTestTimeout)
	flags.withCacheDir()
	flags.withTools()
	changedFile := flags.String("changed", "", "JSON file of the lines the change adds (dupes.schema.json, $defs/changed; required)")
	coverageFile := flags.String("coverage", "", "JSON file of the coverage of the last test run (mutation.schema.json, $defs/coverage; required)")
	maxMutants := flags.Int("max-mutants", mutate.DefaultMaxMutants, "most mutants tested; above it, a sample round-robin across functions")
	jobs := flags.Int("jobs", 0, "mutants tested at the same time (default: half the CPUs)")
	minTimeout := flags.Duration("min-mutant-timeout", mutate.DefaultMinMutantTimeout, "shortest time limit of a mutant's tests")
	progressFile := flags.String("progress", "", "file to keep up to date with the progress of the run")
	offline := flags.Bool("offline", false, "no network: never download modules")
	if !a.parse(flags, args) {
		return exitUsage
	}
	if *maxMutants < 1 || *jobs < 0 || *minTimeout <= 0 {
		fmt.Fprintln(a.stderr, "mutate: --max-mutants must be at least 1, --jobs not negative and --min-mutant-timeout positive")
		return exitUsage
	}
	// mutate tests every added line: the exclude of --changed is for dupes.
	added, _, err := readChanged(*changedFile)
	if err != nil {
		a.log.Error("read changed lines", "file", *changedFile, "error", err.Error())
		fmt.Fprintf(a.stderr, "mutate: cannot read the changed lines: %v\n", err)
		return exitUsage
	}
	coverage, err := readCoverage(*coverageFile)
	if err != nil {
		a.log.Error("read coverage", "file", *coverageFile, "error", err.Error())
		fmt.Fprintf(a.stderr, "mutate: cannot read the coverage: %v\n", err)
		return exitUsage
	}
	engine := protocol.Analyzer{Name: mutate.EngineName, Version: Version}
	start := time.Now()
	// The runner enforces the timeout itself, so that it can still report
	// the mutants it tested when the time is up.
	res, err := a.mutations.Run(context.Background(), mutate.Options{
		Dir: *flags.dir, Timeout: *flags.timeout, Added: added, Coverage: coverage, CacheDir: flags.cacheDirValue(),
		MaxMutants: *maxMutants, Jobs: *jobs, MinMutantTimeout: *minTimeout, Progress: *progressFile, Offline: *offline,
		Engine: engine,
	})
	if err != nil {
		a.log.Error("mutation run failed", "dir", *flags.dir, "error", err.Error())
		return exitFailed
	}
	doc := protocol.MutationRun{
		Protocol:    protocol.Version,
		Language:    "go",
		Runner:      protocol.Analyzer{Name: Name, Version: Version},
		Engine:      engine,
		Mutants:     res.Mutants,
		Summary:     res.Summary,
		Diagnostics: res.Diagnostics,
	}
	if code := a.write(doc); code != exitOK {
		return code
	}
	a.log.Info("mutation run done", "elapsed", time.Since(start).Round(time.Millisecond).String(),
		"mutants", len(res.Mutants), "tested", res.Tested, "killed", res.Summary.Killed, "survived", res.Summary.Survived,
		"timeout", res.Summary.Timeout, "error", res.Summary.Error, "cached", res.Summary.Cached)
	return exitOK
}

// readCoverage reads a --coverage file. Unknown fields, a protocol other
// than 1 and a block whose lines are not from 1, its start not after its
// end, are errors.
func readCoverage(file string) ([]protocol.CoverageBlock, error) {
	if file == "" {
		return nil, errors.New("--coverage is required")
	}
	data, err := os.ReadFile(file)
	if err != nil {
		return nil, err
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	var coverage protocol.MutationCoverage
	if err := dec.Decode(&coverage); err != nil {
		return nil, err
	}
	if coverage.Protocol != protocol.Version {
		return nil, fmt.Errorf("protocol %d is not supported", coverage.Protocol)
	}
	if coverage.Blocks == nil {
		return nil, errors.New("blocks is missing")
	}
	for _, b := range coverage.Blocks {
		if b.StartLine < 1 || b.EndLine < b.StartLine || b.Count < 0 || b.Statements < 0 {
			return nil, fmt.Errorf("%s: %d-%d is not a block", b.File, b.StartLine, b.EndLine)
		}
	}
	return coverage.Blocks, nil
}

// readChanged reads a --changed file: the added lines and the files dupes
// leaves out (none when exclude is absent). Unknown fields, a protocol other
// than 1 and a range that is not two lines from 1, its start not after its
// end, are errors.
func readChanged(file string) (dupes.Added, dupes.Exclude, error) {
	if file == "" {
		return nil, dupes.Exclude{}, errors.New("--changed is required")
	}
	data, err := os.ReadFile(file)
	if err != nil {
		return nil, dupes.Exclude{}, err
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	var changed protocol.Changed
	if err := dec.Decode(&changed); err != nil {
		return nil, dupes.Exclude{}, err
	}
	if changed.Protocol != protocol.Version {
		return nil, dupes.Exclude{}, fmt.Errorf("protocol %d is not supported", changed.Protocol)
	}
	if changed.Files == nil {
		return nil, dupes.Exclude{}, errors.New("files is missing")
	}
	added := dupes.Added{}
	for _, f := range changed.Files {
		if f.Added == nil {
			return nil, dupes.Exclude{}, fmt.Errorf("%s: added is missing", f.Path)
		}
		key := path.Clean(f.Path)
		for _, r := range f.Added {
			if len(r) != 2 || r[0] < 1 || r[1] < r[0] {
				return nil, dupes.Exclude{}, fmt.Errorf("%s: %v is not a range of lines", f.Path, r)
			}
			added[key] = append(added[key], [2]int{r[0], r[1]})
		}
	}
	var exclude dupes.Exclude
	if changed.Exclude != nil {
		exclude = dupes.Exclude{Dirs: changed.Exclude.Dirs, Extensions: changed.Exclude.Extensions}
	}
	return added, exclude, nil
}
