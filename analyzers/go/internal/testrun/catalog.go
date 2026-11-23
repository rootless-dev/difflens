package testrun

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io"
	"path"
	"path/filepath"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/funcindex"
	"github.com/rootless-dev/difflens/analyzers/go/internal/gocmd"
)

// listedPackage is what `go list -json` says about one package.
type listedPackage struct {
	ImportPath   string
	Dir          string
	Name         string
	GoFiles      []string
	CgoFiles     []string
	TestGoFiles  []string
	XTestGoFiles []string
}

// listFields are the fields asked of go list.
const listFields = "ImportPath,Dir,Name,GoFiles,CgoFiles,TestGoFiles,XTestGoFiles"

// catalog knows the packages of the tree: where their files are, under which
// container analyze lists them, and where their test functions are declared.
type catalog struct {
	tree      analyzer.Tree
	pkgs      map[string]*catalogPackage // by import path
	fset      *token.FileSet
	generated map[string]bool // by absolute file name
}

type catalogPackage struct {
	listedPackage
	container string
	tests     *funcindex.Index // test files, indexed on first use
}

func newCatalog(tree analyzer.Tree) *catalog {
	return &catalog{tree: tree, pkgs: map[string]*catalogPackage{}, fset: token.NewFileSet(), generated: map[string]bool{}}
}

// list adds the packages matching patterns, listed by go list in dir with
// env, and returns their import paths in the order go list gave them.
func (c *catalog) list(ctx context.Context, dir string, env, patterns []string) ([]string, error) {
	args := append([]string{"list", "-e", "-json=" + listFields}, patterns...)
	res, err := gocmd.Run(ctx, dir, env, "go", args...)
	if err != nil {
		return nil, err
	}
	var listed []string
	dec := json.NewDecoder(bytes.NewReader(res.Stdout))
	for {
		var p listedPackage
		if err := dec.Decode(&p); errors.Is(err, io.EOF) {
			break
		} else if err != nil {
			return listed, fmt.Errorf("go list: %w", err)
		}
		if c.add(p) {
			listed = append(listed, p.ImportPath)
		}
	}
	if res.ExitCode != 0 {
		return listed, fmt.Errorf("go list: %s", gocmd.FirstLine(res.Stderr))
	}
	return listed, nil
}

// add catalogs p and reports whether it could.
func (c *catalog) add(p listedPackage) bool {
	if p.ImportPath == "" || p.Dir == "" {
		return false
	}
	first := ""
	for _, files := range [][]string{p.GoFiles, p.CgoFiles, p.TestGoFiles} {
		if len(files) > 0 {
			first = files[0]
			break
		}
	}
	name := p.Name
	if first == "" && len(p.XTestGoFiles) > 0 {
		// Only an external test package: analyze sees just that one.
		first = p.XTestGoFiles[0]
		name = c.packageName(filepath.Join(p.Dir, first), p.Name+"_test")
	}
	rel, _ := c.tree.Rel(filepath.Join(p.Dir, first))
	c.pkgs[p.ImportPath] = &catalogPackage{listedPackage: p, container: funcindex.Container(name, rel)}
	return true
}

// container returns the container of the package with import path pkg, as
// analyze writes it.
func (c *catalog) container(pkg string) string {
	if p := c.pkgs[pkg]; p != nil {
		return p.container
	}
	return pkg
}

// testFunc returns the function declaring the top-level test of a test or
// subtest name in package pkg: a function of its _test.go files, in the
// package itself or in its external test package.
func (c *catalog) testFunc(pkg, name string) (*funcindex.Func, bool) {
	p := c.pkgs[pkg]
	if p == nil {
		return nil, false
	}
	if p.tests == nil {
		p.tests = c.indexTests(p)
	}
	top, _, _ := strings.Cut(name, "/")
	for _, id := range []string{pkg + "." + top, pkg + "_test." + top} {
		if f, ok := p.tests.ByID(id); ok && f.IsTest() {
			return f, true
		}
	}
	return nil, false
}

// indexTests indexes the test files of p the way analyze indexes them: the
// internal test files under the package's import path and container, the
// external ones under "<import path>_test".
func (c *catalog) indexTests(p *catalogPackage) *funcindex.Index {
	ix := funcindex.New(c.fset)
	if files := c.parse(p.Dir, p.TestGoFiles); len(files) > 0 {
		ix.Add(funcindex.Package{Path: p.ImportPath, Name: p.Name, Container: p.container, Files: files})
	}
	if files := c.parse(p.Dir, p.XTestGoFiles); len(files) > 0 {
		name := files[0].Syntax.Name.Name
		ix.Add(funcindex.Package{
			Path: p.ImportPath + "_test", Name: name, Container: funcindex.Container(name, files[0].RelPath), Files: files,
		})
	}
	return ix
}

func (c *catalog) parse(dir string, names []string) []funcindex.File {
	var files []funcindex.File
	for _, name := range names {
		abs := filepath.Join(dir, name)
		rel, ok := c.tree.Rel(abs)
		if !ok {
			continue
		}
		f, _ := parser.ParseFile(c.fset, abs, nil, parser.ParseComments|parser.SkipObjectResolution)
		if f == nil || f.Name == nil {
			continue
		}
		files = append(files, funcindex.File{Syntax: f, RelPath: rel})
	}
	return files
}

func (c *catalog) packageName(file, fallback string) string {
	f, err := parser.ParseFile(token.NewFileSet(), file, nil, parser.PackageClauseOnly)
	if err != nil || f.Name == nil {
		return fallback
	}
	return f.Name.Name
}

// sourceFile maps a file name of a coverage profile (an import path and a
// base name, or an absolute name outside modules) to the file relative to
// the tree. It reports false for files outside the tree, test files and
// generated files, which coverage does not report.
func (c *catalog) sourceFile(name string) (string, bool) {
	abs := name
	if !filepath.IsAbs(filepath.FromSlash(name)) {
		p := c.pkgs[path.Dir(name)]
		if p == nil {
			return "", false
		}
		abs = filepath.Join(p.Dir, path.Base(name))
	}
	rel, ok := c.tree.Rel(abs)
	if !ok || strings.HasSuffix(rel, "_test.go") || c.isGenerated(abs) {
		return "", false
	}
	return rel, true
}

// isGenerated applies the convention analyze uses: a "Code generated ...
// DO NOT EDIT." comment before the package clause.
func (c *catalog) isGenerated(file string) bool {
	if gen, ok := c.generated[file]; ok {
		return gen
	}
	f, err := parser.ParseFile(token.NewFileSet(), file, nil, parser.PackageClauseOnly|parser.ParseComments)
	gen := err == nil && ast.IsGenerated(f)
	c.generated[file] = gen
	return gen
}
