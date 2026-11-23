package testrun

import (
	"strings"
)

// Commands that run one test or one package again, as tests.schema.json
// describes `command`: a shell command run from the root of the tree,
//
//	go test [-C <module>] [-race] [-run '<pattern>'] <package>
//
// The package is named relative to the directory go test ran in (the unit's:
// the module, or the workspace at the root of the tree), "./internal/refund"
// or "." for its root package. A unit below the root of the tree, a module
// of a tree with several modules and no go.work, is reached with -C, which
// the go command needs before any other flag. The pattern anchors each level
// of the test's name, '^TestA$' or '^TestA$/^sub$', each level quoted as a
// regular expression, since -run splits patterns at "/" and matches each
// part against the name of the test at that level. Flags the runner only
// needs for itself (-json, -count, coverage, -timeout, -vet) are left out;
// -race is kept when the invocation had the race detector on.

// hierarchy returns the levels of a test's name, from the top-level test
// down: Go names a subtest "<parent>/<name>".
func hierarchy(name string) []string { return strings.Split(name, "/") }

// testCommand returns the command that runs the test (or subtest) name of
// the package with import path pkg on its own, go test having run in
// unitDir.
func (c *catalog) testCommand(unitDir, pkg, name string, race bool) *string {
	levels := hierarchy(name)
	patterns := make([]string, len(levels))
	for i, level := range levels {
		patterns[i] = alternation([]string{level})
	}
	return c.command(unitDir, pkg, race, strings.Join(patterns, "/"))
}

// packageCommand returns the command that runs the tests of the package
// with import path pkg, go test having run in unitDir.
func (c *catalog) packageCommand(unitDir, pkg string, race bool) *string {
	return c.command(unitDir, pkg, race, "")
}

func (c *catalog) command(unitDir, pkg string, race bool, run string) *string {
	if pkg == "" {
		return nil
	}
	unit := relDir(c.tree, unitDir)
	args := []string{"go", "test"}
	if unit != "." {
		args = append(args, "-C", shellQuote(unit))
	}
	if race {
		args = append(args, "-race")
	}
	if run != "" {
		args = append(args, "-run", singleQuote(run))
	}
	args = append(args, shellQuote(c.packageArg(unit, pkg)))
	cmd := strings.Join(args, " ")
	return &cmd
}

// packageArg names the package with import path pkg for a go test run in
// the directory unit (relative to the tree): by its directory relative to
// unit when the catalog knows it, else by its import path, which the go
// command also resolves inside the module.
func (c *catalog) packageArg(unit, pkg string) string {
	p := c.pkgs[pkg]
	if p == nil {
		return pkg
	}
	dir, ok := c.tree.Rel(p.Dir)
	if !ok {
		return pkg
	}
	rel := dir
	if unit != "." {
		if dir == unit {
			rel = "."
		} else if after, found := strings.CutPrefix(dir, unit+"/"); found {
			rel = after
		} else {
			return pkg
		}
	}
	if rel == "." {
		return "."
	}
	return "./" + rel
}

// shellQuote returns s as one word of a POSIX shell: as it is when it holds
// only characters no shell treats specially, single-quoted otherwise.
func shellQuote(s string) string {
	if s != "" && strings.Trim(s, "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-./=:@%+,") == "" {
		return s
	}
	return singleQuote(s)
}

// singleQuote single-quotes s for a POSIX shell: a single quote inside ends
// the quoting, is written escaped (\') and the quoting starts again.
func singleQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}
