// Package dupes finds the duplicated code a review introduces in a Go source
// tree, as analyzers/protocol/dupes.schema.json describes; it implements the
// dupes command:
//
//	difflens-analyzer-go dupes --dir <tree> --changed <file> [--timeout 120s]
//	    [--min-tokens 50] [--min-lines 6]
//
// --changed names the JSON file of the lines the review adds, per file, and
// of the files to leave out ($defs/changed of the schema). A clone is at
// least --min-tokens normalized tokens long, and each of its two copies spans
// at least --min-lines lines. The search reads the files only: it never runs
// project code, never runs the go command and never writes inside the tree.
//
// Files: every *.go file under the tree, test files included, except
//   - those in the folders the analyzer never loads: vendor, testdata,
//     node_modules and folders whose name starts with "." or "_";
//   - those the exclude of --changed names: a folder of the relative path
//     named one of its dirs, or a file name ending with one of its
//     extensions, both ignoring ASCII case;
//   - generated files, by the rule analyze uses: a "// Code generated ...
//     DO NOT EDIT." comment before the package clause (ast.IsGenerated);
//   - files over 1 MiB.
//
// Go has no minified files, so that rule of the schema does not apply.
//
// Scopes: a file named *_test.go is test code, as analyze classifies it;
// every other file is production code. The search below runs once on the
// production files and once on the test files, so a clone never pairs test
// code with production code, and every group carries its scope. Production
// groups come first, then test groups; at most 500 groups are reported per
// scope.
//
// Tokens come from go/scanner with comments dropped. The semicolons the
// scanner inserts at line ends are kept, as ";", so that line structure
// counts. Identifiers become one placeholder, string and character literals
// another and number literals (integer, floating-point and imaginary) a
// third; keywords, operators and punctuation are kept as they are. A raw
// string literal spans every line it covers.
//
// Imports: an import declaration, from the import keyword through its
// closing ")" or the end of its single spec, with the semicolon that ends it,
// is left out, and so is the package clause. Both split the file: no clone
// spans them.
//
// The search follows the steps of the schema: hash every window of
// --min-tokens tokens that touches an added line with a polynomial rolling
// hash, group every window of the tree under those hashes (groups of more
// than 64 windows are boilerplate and dropped), check the tokens of each
// pair of a group really are equal, extend it into a maximal pair, classify
// its sides as new or existing, merge the new sides into blocks and group
// the blocks. A pair is extended once per diagonal (the two files and the
// distance between the copies), so a long clone costs time linear in its
// length rather than quadratic.
package dupes

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// Default minimums of a clone.
const (
	DefaultMinTokens = 50
	DefaultMinLines  = 6
)

// Added is the lines a review adds, per file: inclusive 1-based [start, end]
// ranges keyed by the path relative to the tree, with "/" separators.
type Added map[string][][2]int

// Exclude is the files a search leaves out entirely: those with a folder of
// their path relative to the tree (not the file name) named one of Dirs, and
// those whose file name ends with one of Extensions, both compared ignoring
// ASCII case. Empty entries match nothing. The zero value leaves nothing out.
type Exclude struct {
	Dirs       []string
	Extensions []string
}

// Options are the parameters of a search. Both minimums must be at least 1.
type Options struct {
	MinTokens int
	MinLines  int
	Added     Added
	Exclude   Exclude
}

// Result is what a search found.
type Result struct {
	Duplicates  []protocol.Duplicate
	Diagnostics []string
}

// timeoutMessage is the diagnostic of a search that ran out of time.
const timeoutMessage = "the time limit was reached before the search finished; only the duplicates found so far are reported"

// Search looks for the duplicates the added lines introduce in the tree at
// dir within ctx's deadline. When the deadline passes, it reports what it
// found so far with a diagnostic. It fails only when dir is not a directory
// or cannot be walked.
func Search(ctx context.Context, log logging.Logger, dir string, opts Options) (*Result, error) {
	if opts.MinTokens < 1 || opts.MinLines < 1 {
		return nil, errors.New("the minimum tokens and lines must be at least 1")
	}
	root, err := openRoot(dir)
	if err != nil {
		return nil, err
	}
	res := &Result{Duplicates: []protocol.Duplicate{}, Diagnostics: []string{}}
	files, err := loadFiles(ctx, log, root, opts.Added, opts.Exclude, res)
	if err != nil {
		return nil, err
	}
	production, tests := splitScopes(files)
	// Production groups first, then test groups.
	for _, s := range []struct {
		name  string
		files []*file
	}{{protocol.ScopeProduction, production}, {protocol.ScopeTest, tests}} {
		dupes := searchScope(ctx, log, s.name, s.files, opts)
		if len(dupes) > maxDuplicates {
			res.Diagnostics = append(res.Diagnostics, fmt.Sprintf("%d duplicates found in %s code; only the first %d are reported",
				len(dupes), s.name, maxDuplicates))
			dupes = dupes[:maxDuplicates]
		}
		res.Duplicates = append(res.Duplicates, dupes...)
	}
	if ctx.Err() != nil {
		res.Diagnostics = append(res.Diagnostics, timeoutMessage)
	}
	return res, nil
}

// isTestFile reports whether the file at the relative path rel is test code,
// by the rule analyze uses: its name ends with _test.go.
func isTestFile(rel string) bool { return strings.HasSuffix(rel, "_test.go") }

// splitScopes partitions files, keeping their order, into production files
// and test files.
func splitScopes(files []*file) (production, tests []*file) {
	for _, f := range files {
		if isTestFile(f.path) {
			tests = append(tests, f)
		} else {
			production = append(production, f)
		}
	}
	return production, tests
}

// searchScope runs the search on the files of one scope only, so that no
// clone pairs code of two scopes, and returns its groups, sorted, each
// carrying scope. It stops at the deadline of ctx with what it found.
func searchScope(ctx context.Context, log logging.Logger, scope string, files []*file, opts Options) []protocol.Duplicate {
	var pairs []pair
	if ctx.Err() == nil && len(files) > 0 {
		groups := candidates(ctx, files, opts.MinTokens)
		pairs = findPairs(ctx, files, groups, opts.MinTokens)
	}
	dupes := duplicates(files, classify(files, pairs, opts.MinLines))
	for i := range dupes {
		dupes[i].Scope = scope
	}
	log.Debug("duplicates searched", "scope", scope, "files", len(files), "pairs", len(pairs), "duplicates", len(dupes))
	return dupes
}

// openRoot checks that dir is a directory and returns its absolute,
// symlink-free form, so that the walk enters it even when it is a link.
func openRoot(dir string) (string, error) {
	info, err := os.Stat(dir)
	if err != nil {
		return "", err
	}
	if !info.IsDir() {
		return "", fmt.Errorf("%s is not a directory", dir)
	}
	abs, err := filepath.Abs(dir)
	if err != nil {
		return "", fmt.Errorf("resolve %s: %w", dir, err)
	}
	if real, err := filepath.EvalSymlinks(abs); err == nil {
		return real, nil
	}
	return abs, nil
}
