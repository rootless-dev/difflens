// Package entrypoints finds the functions through which a Go program is
// reached from outside: main, tests, HTTP routes and CLI commands, and the
// registrations that hand functions to a framework which invokes them later
// (routes, middlewares, route groups, commands).
//
// Each way of being reached is a small Detector; Default lists them. Unknown
// frameworks produce nothing: detectors only report registrations they
// recognise by the callee's import path, or by a router interface of the tree
// that a recognised router satisfies.
package entrypoints

import (
	"go/ast"
	"go/constant"
	"go/token"
	"go/types"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// Package is what a detector inspects: one type-checked package.
type Package struct {
	Path    string // import path
	Name    string // package name
	Fset    *token.FileSet
	Files   []File
	Info    *types.Info    // may be partial when the package has type errors
	Types   *types.Package // may be nil
	Program *Program       // what is known about the whole load; may be nil
}

// File is one source file of a package.
type File struct {
	Syntax  *ast.File
	RelPath string // relative to the analysed tree, with "/"
	IsTest  bool
}

// Resolver maps a declaration position (a function name, or the "func"
// keyword of a function literal) to the id of an indexed function.
type Resolver interface {
	FuncIDAt(pos token.Pos) (string, bool)
}

// Found is what a detector reports about a function: an entry point, a
// registration of it, or both.
type Found struct {
	FuncID string
	// EntryPoint has an empty Kind when the function is registered without
	// becoming an entry point, such as a middleware.
	EntryPoint protocol.EntryPoint
	// Registration is nil when the function is not handed over by another
	// function of the tree, such as main or a command declared at package
	// level.
	Registration *Registration
}

// Registration is a statement inside a function of the tree that hands
// another function to a mechanism that will invoke it.
type Registration struct {
	Caller   string // id of the function containing the statement
	Line     int    // line of the registration call
	Evidence string // such as "route POST /refunds" or "middleware"
	// Value is the expression that denotes the registered function there (a
	// function value or literal), so that it is not also reported as a
	// reference; token.NoPos when none does, such as for a closure returned by
	// a constructor.
	Value token.Pos
}

// Detector finds one kind of entry point in a package.
type Detector interface {
	Detect(pkg *Package, funcs Resolver) []Found
}

// Default returns every detector, in the order they run.
func Default() []Detector {
	return []Detector{
		MainDetector{},
		TestDetector{},
		HTTPDetector{},
		CobraDetector{},
	}
}

// constString returns the value of a constant string expression, such as a
// literal or a named constant.
func constString(info *types.Info, expr ast.Expr) (string, bool) {
	if info == nil {
		return "", false
	}
	tv, ok := info.Types[expr]
	if !ok || tv.Value == nil || tv.Value.Kind() != constant.String {
		return "", false
	}
	return constant.StringVal(tv.Value), true
}

// target is an in-tree function a registration hands over.
type target struct {
	id  string
	pos token.Pos // the expression denoting it at the registration, NoPos for none
	// constructor is set when the function is not the registered value
	// itself but the call that builds it, whose result could not be resolved.
	constructor bool
}

// handlerFuncs returns the in-tree functions a handler expression denotes: a
// function literal, a function or method value, a conversion such as
// http.HandlerFunc(f), or, when serveMethod is set, a value whose type has
// that method in the source tree (such as ServeHTTP).
func handlerFuncs(info *types.Info, expr ast.Expr, funcs Resolver, serveMethod string) []target {
	expr = ast.Unparen(expr)
	switch e := expr.(type) {
	case *ast.FuncLit:
		if id, ok := funcs.FuncIDAt(e.Type.Func); ok {
			return []target{{id: id, pos: e.Pos()}}
		}
		return nil
	case *ast.CallExpr:
		if isConversion(info, e) {
			return handlerFuncs(info, e.Args[0], funcs, serveMethod)
		}
	case *ast.Ident, *ast.SelectorExpr:
		if fn := funcObject(info, e); fn != nil {
			if id, ok := funcs.FuncIDAt(fn.Origin().Pos()); ok {
				return []target{{id: id, pos: e.Pos()}}
			}
			return nil
		}
	}
	if serveMethod == "" || info == nil {
		return nil
	}
	t := info.TypeOf(expr)
	if t == nil {
		return nil
	}
	obj, _, _ := types.LookupFieldOrMethod(t, true, nil, serveMethod)
	if fn, ok := obj.(*types.Func); ok {
		if id, ok := funcs.FuncIDAt(fn.Origin().Pos()); ok {
			return []target{{id: id}}
		}
	}
	return nil
}

// isConversion reports whether call converts its single argument to a type,
// as in http.HandlerFunc(f).
func isConversion(info *types.Info, call *ast.CallExpr) bool {
	if info == nil || len(call.Args) != 1 {
		return false
	}
	tv, ok := info.Types[call.Fun]
	return ok && tv.IsType()
}

// EnclosingFunc returns the id of the innermost indexed function containing
// the last node of stack, a path from the file down to that node. A function
// literal at the end of the stack is not its own enclosing function.
func EnclosingFunc(stack []ast.Node, funcs Resolver) (string, bool) {
	for i := len(stack) - 2; i >= 0; i-- {
		switch n := stack[i].(type) {
		case *ast.FuncLit:
			return funcs.FuncIDAt(n.Type.Func)
		case *ast.FuncDecl:
			return funcs.FuncIDAt(n.Name.Pos())
		}
	}
	return "", false
}

// registrationAt returns a registration made at line inside the function of
// stack, or nil when the statement is outside any function of the tree.
func registrationAt(stack []ast.Node, funcs Resolver, line int, evidence string, t target) *Registration {
	caller, ok := EnclosingFunc(stack, funcs)
	if !ok || caller == t.id {
		return nil
	}
	return &Registration{Caller: caller, Line: line, Evidence: evidence, Value: t.pos}
}

// funcObject returns the function or method an identifier or selector refers
// to, or nil if it refers to anything else (a variable, a field).
func funcObject(info *types.Info, expr ast.Expr) *types.Func {
	if info == nil {
		return nil
	}
	var id *ast.Ident
	switch e := expr.(type) {
	case *ast.Ident:
		id = e
	case *ast.SelectorExpr:
		id = e.Sel
	}
	if id == nil {
		return nil
	}
	fn, _ := info.Uses[id].(*types.Func)
	return fn
}

// calleeFunc returns the function or method a call expression calls, if it
// is statically known (interface methods included).
func calleeFunc(info *types.Info, call *ast.CallExpr) *types.Func {
	if info == nil {
		return nil
	}
	switch fun := ast.Unparen(call.Fun).(type) {
	case *ast.Ident:
		fn, _ := info.Uses[fun].(*types.Func)
		return fn
	case *ast.SelectorExpr:
		fn, _ := info.Uses[fun.Sel].(*types.Func)
		return fn
	case *ast.IndexExpr, *ast.IndexListExpr:
		return nil
	}
	return nil
}

// pkgPathOf returns the import path of the package declaring fn.
func pkgPathOf(fn *types.Func) string {
	if fn.Pkg() == nil {
		return ""
	}
	return fn.Pkg().Path()
}

// isModuleRoot reports whether path is the root package of module, in any
// major version: "github.com/go-chi/chi" matches ".../chi" and ".../chi/v5".
func isModuleRoot(path, module string) bool {
	if path == module {
		return true
	}
	rest, ok := strings.CutPrefix(path, module+"/v")
	if !ok || rest == "" {
		return false
	}
	for _, r := range rest {
		if r < '0' || r > '9' {
			return false
		}
	}
	return true
}

func position(pkg *Package, file File, pos token.Pos) (string, int) {
	return file.RelPath, pkg.Fset.PositionFor(pos, false).Line
}
