// Package funcindex lists the functions, methods and closures declared in Go
// syntax and gives each a stable id.
//
// Ids are built from syntax only, so they do not depend on type checking, on
// SSA naming or on the order of unrelated declarations:
//
//	function     <import path>.<Name>             example.com/p.Run
//	method       <import path>.(*T).M or .T.M     example.com/p.(*Service).Request
//	init         <import path>.init#<file>#<k>    example.com/p.init#db.go#1 (k-th init in that file)
//	closure      <parent id>$<k>                  example.com/p.Run$1, ...Run$1$2
//	var closure  <import path>.<var>$<k>          example.com/p.handler$1
//
// A closure's k is its position among the closures written directly inside
// its parent, in source order, so edits elsewhere in the file (or in other
// functions) never renumber it.
package funcindex

import (
	"go/ast"
	"go/token"
	"path"
	"strconv"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/complexity"
)

// Func is one indexed function.
type Func struct {
	ID            string
	Name          string
	QualifiedName string
	Container     string
	File          string
	StartLine     int
	EndLine       int
	Generated     bool
	Complexity    int // 0 when the function has no body
}

// Package is the syntax of one Go package to index.
type Package struct {
	Path      string // import path
	Name      string // package name
	Container string // display container, such as "internal/refund"
	Files     []File
}

// Container returns the display container of a package named name with a
// file at relFile (relative to the analysed tree): the file's directory, or
// the package name when the file is at the root of the tree or unknown. An
// external test package at the root is therefore "<name>_test".
func Container(name, relFile string) string {
	if relFile == "" {
		return name
	}
	if dir := path.Dir(relFile); dir != "." {
		return dir
	}
	return name
}

// File is one parsed file and its path relative to the analysed tree.
type File struct {
	Syntax  *ast.File
	RelPath string
}

type posKey struct {
	file   string
	offset int
}

// Index maps declaration positions to functions.
type Index struct {
	fset  *token.FileSet
	byKey map[posKey]*Func
	byID  map[string]*Func
	order []*Func
}

// New returns an empty index for syntax parsed with fset.
func New(fset *token.FileSet) *Index {
	return &Index{fset: fset, byKey: map[posKey]*Func{}, byID: map[string]*Func{}}
}

// Funcs returns the indexed functions, one per id, in insertion order.
func (ix *Index) Funcs() []*Func { return ix.order }

// At returns the function declared at pos: the name of a declared function or
// method, or the "func" keyword of a function literal. Positions are compared
// by file name and offset, so the same file parsed twice (for instance in a
// package and in its test variant) resolves to the same function.
func (ix *Index) At(pos token.Pos) (*Func, bool) {
	if !pos.IsValid() {
		return nil, false
	}
	f, ok := ix.byKey[ix.key(pos)]
	return f, ok
}

// ByID returns the function with the given id.
func (ix *Index) ByID(id string) (*Func, bool) {
	f, ok := ix.byID[id]
	return f, ok
}

// IsTest reports whether f is declared in a _test.go file.
func (f *Func) IsTest() bool { return strings.HasSuffix(f.File, "_test.go") }

// FuncIDAt is At reduced to the id, for callers that only need the id.
func (ix *Index) FuncIDAt(pos token.Pos) (string, bool) {
	f, ok := ix.At(pos)
	if !ok {
		return "", false
	}
	return f.ID, true
}

func (ix *Index) key(pos token.Pos) posKey {
	p := ix.fset.PositionFor(pos, false)
	return posKey{file: p.Filename, offset: p.Offset}
}

// Add indexes every function of pkg. Adding the same file twice is harmless.
func (ix *Index) Add(pkg Package) {
	b := builder{ix: ix, pkg: pkg, counters: map[string]int{}}
	for _, f := range pkg.Files {
		b.file = f
		b.generated = ast.IsGenerated(f.Syntax)
		inits := 0
		for _, decl := range f.Syntax.Decls {
			switch decl := decl.(type) {
			case *ast.FuncDecl:
				if decl.Name.Name == "_" {
					continue
				}
				id, name := b.declName(decl)
				if decl.Recv == nil && decl.Name.Name == "init" {
					inits++
					suffix := "#" + path.Base(f.RelPath) + "#" + strconv.Itoa(inits)
					id, name = id+suffix, name+suffix
				}
				b.add(decl.Name.Pos(), decl.Pos(), decl.End(), id, name, decl.Body)
				b.closures(decl.Body, id, name)
			case *ast.GenDecl:
				if decl.Tok != token.VAR {
					continue
				}
				for _, spec := range decl.Specs {
					vs := spec.(*ast.ValueSpec)
					for i, value := range vs.Values {
						varName := vs.Names[0].Name
						if len(vs.Names) == len(vs.Values) {
							varName = vs.Names[i].Name
						}
						b.closures(value, pkg.Path+"."+varName, varName)
					}
				}
			}
		}
	}
}

type builder struct {
	ix        *Index
	pkg       Package
	file      File
	generated bool
	counters  map[string]int // closures seen so far per parent id
}

func (b *builder) declName(decl *ast.FuncDecl) (id, name string) {
	if decl.Recv == nil || len(decl.Recv.List) == 0 {
		return b.pkg.Path + "." + decl.Name.Name, decl.Name.Name
	}
	recv, pointer := receiverName(decl.Recv.List[0].Type)
	name = recv + "." + decl.Name.Name
	if pointer {
		return b.pkg.Path + ".(*" + recv + ")." + decl.Name.Name, name
	}
	return b.pkg.Path + "." + name, name
}

// receiverName returns the receiver's type name without type parameters.
func receiverName(expr ast.Expr) (name string, pointer bool) {
	for {
		switch e := expr.(type) {
		case *ast.ParenExpr:
			expr = e.X
		case *ast.StarExpr:
			pointer = true
			expr = e.X
		case *ast.IndexExpr:
			expr = e.X
		case *ast.IndexListExpr:
			expr = e.X
		case *ast.Ident:
			return e.Name, pointer
		default:
			return "?", pointer
		}
	}
}

// closures indexes the function literals written directly inside node, then
// recursively the ones inside each of them.
func (b *builder) closures(node ast.Node, parentID, parentName string) {
	if node == nil {
		return
	}
	ast.Inspect(node, func(n ast.Node) bool {
		lit, ok := n.(*ast.FuncLit)
		if !ok {
			return true
		}
		b.counters[parentID]++
		suffix := "$" + strconv.Itoa(b.counters[parentID])
		id, name := parentID+suffix, parentName+suffix
		b.add(lit.Type.Func, lit.Pos(), lit.End(), id, name, lit.Body)
		b.closures(lit.Body, id, name)
		return false
	})
}

func (b *builder) add(key, start, end token.Pos, id, name string, body *ast.BlockStmt) {
	k := b.ix.key(key)
	if _, ok := b.ix.byKey[k]; ok {
		return
	}
	if existing, ok := b.ix.byID[id]; ok {
		b.ix.byKey[k] = existing
		return
	}
	startPos := b.ix.fset.PositionFor(start, false)
	endPos := b.ix.fset.PositionFor(end, false)
	f := &Func{
		ID:            id,
		Name:          name,
		QualifiedName: b.pkg.Name + "." + name,
		Container:     b.pkg.Container,
		File:          b.file.RelPath,
		StartLine:     startPos.Line,
		EndLine:       endPos.Line,
		Generated:     b.generated,
		Complexity:    complexity.Cyclomatic(body),
	}
	b.ix.byKey[k] = f
	b.ix.byID[id] = f
	b.ix.order = append(b.ix.order, f)
}
