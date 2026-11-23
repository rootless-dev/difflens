package testrun

import (
	"slices"
	"testing"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

func TestHierarchy(t *testing.T) {
	for name, want := range map[string][]string{
		"TestA":            {"TestA"},
		"TestA/b":          {"TestA", "b"},
		"TestA/b/c_d":      {"TestA", "b", "c_d"},
		"TestA/#00":        {"TestA", "#00"},
		"ExampleAdd":       {"ExampleAdd"},
		"TestA/with_space": {"TestA", "with_space"},
	} {
		if got := hierarchy(name); !slices.Equal(got, want) {
			t.Errorf("%s: got %q, want %q", name, got, want)
		}
	}
}

// catalogOf returns a catalog of a tree at /src/tree with the packages
// given by import path and directory relative to the tree.
func catalogOf(t *testing.T, dirs map[string]string) *catalog {
	t.Helper()
	tree, err := analyzer.NewTree("/src/tree")
	if err != nil {
		t.Fatal(err)
	}
	c := newCatalog(tree)
	for path, dir := range dirs {
		c.pkgs[path] = &catalogPackage{listedPackage: listedPackage{ImportPath: path, Dir: "/src/tree/" + dir}}
	}
	return c
}

func TestTestCommands(t *testing.T) {
	c := catalogOf(t, map[string]string{
		"example.com/m":            ".",
		"example.com/m/internal/r": "internal/r",
		"example.com/b/pkg":        "mods/b/pkg",
		"example.com/b":            "mods/b",
	})
	for _, tc := range []struct {
		unit, pkg, name string
		race            bool
		want            string
	}{
		{"/src/tree", "example.com/m/internal/r", "TestA", true, `go test -race -run '^TestA$' ./internal/r`},
		{"/src/tree", "example.com/m/internal/r", "TestA", false, `go test -run '^TestA$' ./internal/r`},
		{"/src/tree", "example.com/m/internal/r", "TestA/gateway_declines", true, `go test -race -run '^TestA$/^gateway_declines$' ./internal/r`},
		{"/src/tree", "example.com/m/internal/r", "TestA/b/c", false, `go test -run '^TestA$/^b$/^c$' ./internal/r`},
		// Every metacharacter is escaped, and a quote leaves the single quotes.
		{"/src/tree", "example.com/m", "TestA/a.b*c+d?e^f$g{h}i(j)k|l[m]n\\o", false,
			`go test -run '^TestA$/^a\.b\*c\+d\?e\^f\$g\{h\}i\(j\)k\|l\[m\]n\\o$' .`},
		{"/src/tree", "example.com/m", "TestA/it's", false, `go test -run '^TestA$/^it'\''s$' .`},
		{"/src/tree", "example.com/m", "TestA/#01", false, `go test -run '^TestA$/^#01$' .`},
		// A module below the root of the tree: go test ran in it.
		{"/src/tree/mods/b", "example.com/b/pkg", "TestB", true, `go test -C mods/b -race -run '^TestB$' ./pkg`},
		{"/src/tree/mods/b", "example.com/b", "TestB", false, `go test -C mods/b -run '^TestB$' .`},
		// A package the catalog does not know is named by import path.
		{"/src/tree", "example.com/m/gone", "TestG", false, `go test -run '^TestG$' example.com/m/gone`},
	} {
		got := c.testCommand(tc.unit, tc.pkg, tc.name, tc.race)
		if got == nil || *got != tc.want {
			t.Errorf("%s %s: got %s, want %s", tc.pkg, tc.name, str(got), tc.want)
		}
	}
}

func TestPackageCommands(t *testing.T) {
	c := catalogOf(t, map[string]string{
		"example.com/m":            ".",
		"example.com/m/internal/r": "internal/r",
		"example.com/b/pkg":        "mods/b/pkg",
		"example.com/w/x":          "w/x",
		"example.com/m/odd dir":    "odd dir",
	})
	for _, tc := range []struct {
		unit, pkg string
		race      bool
		want      string
	}{
		{"/src/tree", "example.com/m/internal/r", true, "go test -race ./internal/r"},
		{"/src/tree", "example.com/m/internal/r", false, "go test ./internal/r"},
		{"/src/tree", "example.com/m", true, "go test -race ."},
		{"/src/tree/mods/b", "example.com/b/pkg", false, "go test -C mods/b ./pkg"},
		// A workspace runs at the root, its modules' packages by directory.
		{"/src/tree", "example.com/w/x", false, "go test ./w/x"},
		{"/src/tree", "example.com/m/odd dir", false, "go test './odd dir'"},
	} {
		got := c.packageCommand(tc.unit, tc.pkg, tc.race)
		if got == nil || *got != tc.want {
			t.Errorf("%s: got %s, want %s", tc.pkg, str(got), tc.want)
		}
	}
	if got := c.packageCommand("/src/tree", "", false); got != nil {
		t.Errorf("a package without import path has a command: %s", *got)
	}
}

func TestShellQuote(t *testing.T) {
	for in, want := range map[string]string{
		"./internal/r":  "./internal/r",
		"example.com/m": "example.com/m",
		"a b":           "'a b'",
		"it's":          `'it'\''s'`,
		"$HOME":         "'$HOME'",
		"":              "''",
	} {
		if got := shellQuote(in); got != want {
			t.Errorf("%q: got %s, want %s", in, got, want)
		}
	}
}

// collect writes the hierarchy and the commands of what ran, with -race
// only when the invocation had the race detector.
func TestCollectWritesHierarchyAndCommands(t *testing.T) {
	const pkg = "example.com/m/calc"
	events := stream(t,
		event{Action: "run", Package: pkg, Test: "TestDiv"},
		event{Action: "run", Package: pkg, Test: "TestDiv/by_zero"},
		event{Action: "pass", Package: pkg, Test: "TestDiv/by_zero"},
		event{Action: "pass", Package: pkg, Test: "TestDiv"},
		event{Action: "pass", Package: pkg},
	)
	for _, race := range []bool{true, false} {
		s := newSession()
		s.catalog = catalogOf(t, map[string]string{pkg: "calc"})
		s.collect(unitRun{dir: "/src/tree", race: race, events: collect(t, events)})
		flag := ""
		if race {
			flag = "-race "
		}
		if len(s.result.Tests) != 2 {
			t.Fatalf("tests: %+v", s.result.Tests)
		}
		sub := s.result.Tests[0]
		if !slices.Equal(sub.Hierarchy, []string{"TestDiv", "by_zero"}) || str(sub.Command) != "go test "+flag+"-run '^TestDiv$/^by_zero$' ./calc" {
			t.Errorf("race=%v: subtest %q %s", race, sub.Hierarchy, str(sub.Command))
		}
		if top := s.result.Tests[1]; !slices.Equal(top.Hierarchy, []string{"TestDiv"}) || str(top.Command) != "go test "+flag+"-run '^TestDiv$' ./calc" {
			t.Errorf("race=%v: test %q %s", race, top.Hierarchy, str(top.Command))
		}
		if p := s.result.Packages; len(p) != 1 {
			t.Errorf("race=%v: packages %+v", race, p)
		} else if p[0].Outcome != protocol.OutcomePass || str(p[0].Command) != "go test "+flag+"./calc" {
			t.Errorf("race=%v: package %s %s", race, p[0].Outcome, str(p[0].Command))
		}
	}
}
