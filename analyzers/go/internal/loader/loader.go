// Package loader finds the Go modules in a source tree and loads their
// packages with golang.org/x/tools/go/packages, without writing to the tree.
package loader

import (
	"context"
	"errors"
	"fmt"
	"go/token"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"slices"
	"strings"

	"golang.org/x/mod/modfile"
	"golang.org/x/tools/go/packages"
)

// Unit is one independent load: a workspace (go.work at the root) or a
// module (a directory with go.mod).
type Unit struct {
	Dir      string   // absolute directory the go command runs in
	GoWork   string   // go.work file to use, or "off"
	Vendor   bool     // load dependencies from vendor/ instead of the module cache
	Patterns []string // package patterns, relative to Dir
}

// ErrNoModule is returned when the tree has neither go.work nor go.mod.
var ErrNoModule = errors.New("no go.mod or go.work found")

// Discover returns the units to load under root, sorted by directory. A
// go.work at the root wins and makes one unit covering the modules it uses
// inside the tree; otherwise every go.mod outside vendor, testdata and hidden
// directories makes its own unit.
func Discover(root string) ([]Unit, error) {
	if work := filepath.Join(root, "go.work"); fileExists(work) {
		patterns, err := workspacePatterns(work)
		if err != nil {
			return nil, err
		}
		return []Unit{{Dir: root, GoWork: work, Vendor: fileExists(filepath.Join(root, "vendor", "modules.txt")), Patterns: patterns}}, nil
	}
	var units []Unit
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			name := d.Name()
			if path != root && (name == "vendor" || name == "testdata" || name == "node_modules" ||
				strings.HasPrefix(name, ".") || strings.HasPrefix(name, "_")) {
				return filepath.SkipDir
			}
			return nil
		}
		if d.Name() == "go.mod" {
			dir := filepath.Dir(path)
			units = append(units, Unit{Dir: dir, GoWork: "off", Vendor: fileExists(filepath.Join(dir, "vendor", "modules.txt")), Patterns: []string{"./..."}})
		}
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("scan %s: %w", root, err)
	}
	if len(units) == 0 {
		return nil, ErrNoModule
	}
	slices.SortFunc(units, func(a, b Unit) int { return strings.Compare(a.Dir, b.Dir) })
	return units, nil
}

// workspacePatterns returns one "./<dir>/..." pattern per module the go.work
// file uses inside its own directory; modules outside the tree are skipped.
func workspacePatterns(work string) ([]string, error) {
	data, err := os.ReadFile(work)
	if err != nil {
		return nil, err
	}
	wf, err := modfile.ParseWork(work, data, nil)
	if err != nil {
		return nil, fmt.Errorf("parse %s: %w", work, err)
	}
	var patterns []string
	for _, use := range wf.Use {
		dir := filepath.Clean(filepath.FromSlash(use.Path))
		if filepath.IsAbs(dir) {
			rel, err := filepath.Rel(filepath.Dir(work), dir)
			if err != nil {
				continue
			}
			dir = rel
		}
		if !filepath.IsLocal(dir) && dir != "." {
			continue
		}
		patterns = append(patterns, "./"+path.Join(filepath.ToSlash(dir), "..."))
	}
	if len(patterns) == 0 {
		return nil, fmt.Errorf("%s uses no module inside the tree", work)
	}
	slices.Sort(patterns)
	return slices.Compact(patterns), nil
}

// Mode is what the analysis needs: syntax and types for the packages of the
// tree, tests included. Dependencies are not requested (no NeedDeps), so they
// are type-checked from compiler export data instead of from source, which
// keeps large dependency graphs fast; their function bodies are not needed.
const Mode = packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles |
	packages.NeedImports | packages.NeedTypes | packages.NeedTypesSizes |
	packages.NeedSyntax | packages.NeedTypesInfo | packages.NeedModule

// Load loads every package matching the unit's patterns, tests included. The module's
// go.mod and go.sum are used read-only (-mod=readonly, or -mod=vendor when
// the module vendors its dependencies); downloads go to the user's module
// cache, never into the tree. Package errors are left on the packages; an
// error is returned only when the go command itself could not run.
func Load(ctx context.Context, unit Unit) ([]*packages.Package, *token.FileSet, error) {
	fset := token.NewFileSet()
	cfg := &packages.Config{
		Context: ctx,
		Mode:    Mode,
		Dir:     unit.Dir,
		Env:     environment(os.Environ(), unit),
		Tests:   true,
		Fset:    fset,
	}
	pkgs, err := packages.Load(cfg, unit.Patterns...)
	if err != nil {
		return nil, nil, err
	}
	return pkgs, fset, nil
}

// LoadPackages loads the packages matching patterns (relative to the unit's
// directory), without their tests, with the go command running with env,
// which the caller prepared with Unit.Env. Like Load, package errors are
// left on the packages.
func LoadPackages(ctx context.Context, unit Unit, env, patterns []string) ([]*packages.Package, *token.FileSet, error) {
	fset := token.NewFileSet()
	cfg := &packages.Config{Context: ctx, Mode: Mode, Dir: unit.Dir, Env: env, Fset: fset}
	pkgs, err := packages.Load(cfg, patterns...)
	if err != nil {
		return nil, nil, err
	}
	return pkgs, fset, nil
}

// environment returns env with GOFLAGS forcing a read-only module mode and
// GOWORK set for the unit.
func environment(env []string, unit Unit) []string { return unit.Env(env, "") }

// Env returns env prepared to run the go command for the unit: GOWORK and PWD
// set for it and GOFLAGS carrying -mod=<mod> and -buildvcs=false, replacing
// any -mod or -buildvcs flag the user had. An empty mod selects the read-only
// mode: vendor when the unit vendors its dependencies, readonly otherwise.
func (unit Unit) Env(env []string, mod string) []string {
	if mod == "" {
		mod = "readonly"
		if unit.Vendor {
			mod = "vendor"
		}
	}
	var out []string
	// -buildvcs=false: never run the version control tool inside the tree.
	flags := []string{"-mod=" + mod, "-buildvcs=false"}
	for _, kv := range env {
		switch {
		case strings.HasPrefix(kv, "GOFLAGS="):
			for _, f := range strings.Fields(strings.TrimPrefix(kv, "GOFLAGS=")) {
				name, _, _ := strings.Cut(strings.TrimLeft(f, "-"), "=")
				if name != "mod" && name != "buildvcs" {
					flags = append(flags, f)
				}
			}
		case strings.HasPrefix(kv, "GOWORK="), strings.HasPrefix(kv, "PWD="):
		default:
			out = append(out, kv)
		}
	}
	return append(out, "GOFLAGS="+strings.Join(flags, " "), "GOWORK="+unit.GoWork, "PWD="+unit.Dir)
}

func fileExists(path string) bool {
	info, err := os.Stat(path)
	return err == nil && !info.IsDir()
}
