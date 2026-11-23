// Package app wires the Go analyzer together and implements its command line:
//
//	difflens-analyzer-go analyze --dir <source tree> [--timeout 120s]
//	difflens-analyzer-go prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
//	difflens-analyzer-go test --dir <source tree> --timeout <duration> [--cache-dir <folder>] [--offline] [--no-race] [--no-cover]
//	    [--selection <file>] [--progress <file>]
//	difflens-analyzer-go check --dir <source tree> --timeout <duration> [--cache-dir <folder>]
//	difflens-analyzer-go dupes --dir <source tree> --changed <file> [--timeout 120s]
//	    [--min-tokens 50] [--min-lines 6]
//	difflens-analyzer-go mutate --dir <source tree> --timeout <duration> --changed <file> --coverage <file>
//	    [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>] [--min-mutant-timeout 10s]
//	    [--progress <file>] [--offline]
//	difflens-analyzer-go --version
//
// --cache-dir is the core's persistent folder for the project, outside the
// tree. Go's build cache is persistent already and content-addressed, so the
// user's GOCACHE is used, and shared with the developer's own builds; the
// folder holds the build cache (<folder>/go-build) only when GOCACHE is empty
// or "off" (prepare then reports it as writable, and test, check and mutate
// use it). mutate also keeps its results there, in <folder>/mutation/go.
//
// --tools is where prepare installs a mutation engine for the analyzers that
// drive one; Go's mutator is part of the analyzer, so prepare and mutate
// accept the flag and ignore it.
//
// mutate (mutation.schema.json) tests the lines a review adds that the tests
// execute: --changed lists the added lines (dupes.schema.json, $defs/changed)
// and --coverage the blocks of the last test run with coverage
// (mutation.schema.json, $defs/coverage). Each mutant runs the tests of its
// own package only, stopped after max(--min-mutant-timeout, 3 x the
// package's unmutated run); --jobs defaults to half the CPUs. See package
// mutate.
package app

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"time"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/checks"
	"github.com/rootless-dev/difflens/analyzers/go/internal/entrypoints"
	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/mutate"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
	"github.com/rootless-dev/difflens/analyzers/go/internal/testrun"
)

// Analyzer identity reported by --version and in every document.
const (
	Name    = "difflens-analyzer-go"
	Version = "0.7.0"
)

const usage = `usage: %[1]s analyze --dir <source tree> [--timeout 120s]
       %[1]s prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
       %[1]s test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline] [--no-race] [--no-cover]
           [--selection <file>] [--progress <file>]
       %[1]s check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
       %[1]s dupes --dir <source tree> --changed <file> [--timeout 120s]
           [--min-tokens 50] [--min-lines 6]
       %[1]s mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 10m]
           [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]
           [--min-mutant-timeout 10s] [--progress <file>] [--offline]
       %[1]s --version

--cache-dir: persistent folder for the project, outside the tree. It holds
the build cache (<folder>/go-build) only when GOCACHE is empty or "off";
otherwise the user's build cache is used. mutate keeps its results there.
--tools: accepted and ignored (Go's mutator needs no engine).
`

// DefaultTimeout bounds a whole analysis when --timeout is not given.
const DefaultTimeout = 120 * time.Second

// Exit codes.
const (
	exitOK     = 0
	exitFailed = 1
	exitUsage  = 2
)

// Run is the process entry point: it builds the application from the
// environment and runs it with args (without the program name).
func Run(args []string, stdout, stderr io.Writer) int {
	log := logging.New(stderr, Name, os.Getenv("DIFFLENS_LOG_LEVEL"))
	tests := testrun.New(log)
	return New(stdout, stderr, log,
		analyzer.New(log, entrypoints.Default()),
		tests,
		checks.New(log, checks.DefaultFinder),
		mutate.New(log, tests),
	).Run(args)
}

// App is the analyzer command.
type App struct {
	stdout    io.Writer
	stderr    io.Writer
	log       logging.Logger
	analyzer  *analyzer.Analyzer
	tests     *testrun.Runner
	checks    *checks.Runner
	mutations *mutate.Runner
}

// New returns the command writing the document to stdout and usage errors to
// stderr.
func New(stdout, stderr io.Writer, log logging.Logger, a *analyzer.Analyzer, tests *testrun.Runner, checks *checks.Runner, mutations *mutate.Runner) *App {
	return &App{stdout: stdout, stderr: stderr, log: log, analyzer: a, tests: tests, checks: checks, mutations: mutations}
}

// Run executes the command line args and returns the exit code.
func (a *App) Run(args []string) int {
	if len(args) > 0 && (args[0] == "--version" || args[0] == "-version" || args[0] == "version") {
		fmt.Fprintf(a.stdout, "%s %s\n", Name, Version)
		return exitOK
	}
	if len(args) == 0 {
		fmt.Fprintf(a.stderr, usage, Name)
		return exitUsage
	}
	switch args[0] {
	case "analyze":
		return a.runAnalyze(args[1:])
	case "prepare":
		return a.runPrepare(args[1:])
	case "test":
		return a.runTest(args[1:])
	case "check":
		return a.runCheck(args[1:])
	case "dupes":
		return a.runDupes(args[1:])
	case "mutate":
		return a.runMutate(args[1:])
	}
	fmt.Fprintf(a.stderr, usage, Name)
	return exitUsage
}

// commandFlags is the flag set of a command with its --dir and --timeout,
// and --cache-dir for the commands that take it (nil otherwise).
type commandFlags struct {
	*flag.FlagSet
	name     string
	dir      *string
	timeout  *time.Duration
	cacheDir *string
}

func (a *App) newFlags(name string, timeout time.Duration) commandFlags {
	flags := flag.NewFlagSet(name, flag.ContinueOnError)
	flags.SetOutput(a.stderr)
	return commandFlags{
		FlagSet: flags,
		name:    name,
		dir:     flags.String("dir", "", "source tree (required)"),
		timeout: flags.Duration("timeout", timeout, "maximum duration of the whole command"),
	}
}

// withCacheDir adds --cache-dir (see the package documentation).
func (f *commandFlags) withCacheDir() {
	f.cacheDir = f.String("cache-dir", "", "persistent folder for this project, outside the tree; holds the build cache only when GOCACHE is not usable")
}

// withTools adds --tools, accepted and ignored (see the package
// documentation).
func (f *commandFlags) withTools() {
	f.String("tools", "", "folder of the mutation engine; Go needs none, so it is ignored")
}

// parse parses args and reports whether they are valid. --cache-dir, when
// given, is made absolute.
func (a *App) parse(flags commandFlags, args []string) bool {
	if err := flags.Parse(args); err != nil {
		return false
	}
	if *flags.dir == "" || flags.NArg() > 0 || *flags.timeout <= 0 {
		fmt.Fprintf(a.stderr, "%s: --dir is required, --timeout must be positive and no positional arguments are accepted\n", flags.name)
		return false
	}
	if flags.cacheDir != nil && *flags.cacheDir != "" {
		abs, err := filepath.Abs(*flags.cacheDir)
		if err != nil {
			fmt.Fprintf(a.stderr, "%s: --cache-dir: %v\n", flags.name, err)
			return false
		}
		*flags.cacheDir = abs
	}
	return true
}

// cacheDirValue returns the value of --cache-dir, "" when it was not given
// or the command has no such flag.
func (f commandFlags) cacheDirValue() string {
	if f.cacheDir == nil {
		return ""
	}
	return *f.cacheDir
}

// write prints doc as the single JSON document on stdout.
func (a *App) write(doc any) int {
	enc := json.NewEncoder(a.stdout)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(doc); err != nil {
		a.log.Error("write output", "error", err.Error())
		return exitFailed
	}
	return exitOK
}

func (a *App) runAnalyze(args []string) int {
	flags := a.newFlags("analyze", DefaultTimeout)
	if !a.parse(flags, args) {
		return exitUsage
	}
	dir := *flags.dir
	ctx, cancel := context.WithTimeout(context.Background(), *flags.timeout)
	defer cancel()
	start := time.Now()
	doc, err := a.Analyze(ctx, dir)
	if err != nil {
		a.log.Error("analysis failed", "dir", dir, "error", err.Error())
		return exitFailed
	}
	if code := a.write(doc); code != exitOK {
		return code
	}
	a.log.Info("analysis done",
		"elapsed", time.Since(start).Round(time.Millisecond).String(),
		"functions", len(doc.Functions), "calls", len(doc.Calls),
		"externals", len(doc.Externals), "diagnostics", len(doc.Diagnostics))
	return exitOK
}

// Analyze analyses the tree at dir within ctx's deadline.
func (a *App) Analyze(ctx context.Context, dir string) (*protocol.Document, error) {
	tree, units, err := analyzer.OpenTree(dir)
	if err != nil {
		return nil, err
	}

	type outcome struct {
		doc *protocol.Document
		err error
	}
	done := make(chan outcome, 1)
	go func() {
		doc, err := a.analyzeUnits(ctx, tree, units)
		done <- outcome{doc, err}
	}()
	select {
	case o := <-done:
		return o.doc, o.err
	case <-ctx.Done():
		return nil, fmt.Errorf("analysis did not finish in time: %w", ctx.Err())
	}
}

func (a *App) analyzeUnits(ctx context.Context, tree analyzer.Tree, units []loader.Unit) (*protocol.Document, error) {
	var result analyzer.Result
	var loadErrs []error
	loaded := 0
	for _, unit := range units {
		start := time.Now()
		pkgs, fset, err := loader.Load(ctx, unit)
		if err != nil {
			if ctx.Err() != nil {
				return nil, ctx.Err()
			}
			loadErrs = append(loadErrs, fmt.Errorf("load %s: %w", unit.Dir, err))
			result.Diagnostics = append(result.Diagnostics, protocol.Diagnostic{
				Level: protocol.LevelError, Message: fmt.Sprintf("could not load %s: %v", relDir(tree, unit.Dir), err),
			})
			continue
		}
		loaded++
		a.log.Info("packages loaded", "dir", unit.Dir, "packages", len(pkgs), "elapsed", time.Since(start).Round(time.Millisecond).String())
		if len(pkgs) == 0 {
			result.Diagnostics = append(result.Diagnostics, protocol.Diagnostic{
				Level: protocol.LevelWarning, Message: fmt.Sprintf("no Go packages in %s", relDir(tree, unit.Dir)),
			})
			continue
		}
		result.Append(a.analyzer.Analyze(tree, pkgs, fset))
	}
	if loaded == 0 {
		return nil, errors.Join(loadErrs...)
	}
	return document(result), nil
}

// document assembles the output. An external that is a function of another
// module of the tree (a multi-module tree without go.work) is dropped: calls
// to it already carry the function's id.
func document(result analyzer.Result) *protocol.Document {
	doc := &protocol.Document{
		Protocol:    protocol.Version,
		Language:    "go",
		Analyzer:    protocol.Analyzer{Name: Name, Version: Version},
		Functions:   result.Functions,
		Calls:       result.Calls,
		Diagnostics: result.Diagnostics,
	}
	inTree := map[string]bool{}
	for _, f := range result.Functions {
		inTree[f.ID] = true
	}
	for _, ext := range result.Externals {
		if !inTree[ext.ID] {
			doc.Externals = append(doc.Externals, ext)
		}
	}
	doc.Normalize()
	return doc
}

func relDir(tree analyzer.Tree, dir string) string {
	if rel, ok := tree.Rel(dir); ok {
		return rel
	}
	return "."
}
