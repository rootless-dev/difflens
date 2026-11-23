package analyzer

import (
	"go/ast"
	"go/types"

	"golang.org/x/tools/go/packages"

	"github.com/rootless-dev/difflens/analyzers/go/internal/entrypoints"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// edge is a relation's identity without its relation, to find the sites a
// call or a registration already covers.
type edge struct {
	caller, callee string
	line           int
}

// references adds a reference from a function to each function of the tree
// it uses as a value without calling it: a function or method value, a
// method expression or a function literal, given as an argument, stored in a
// field, a variable, a map or a slice, or returned. A value that is
// immediately called is a call; one handed over by a recognised registration
// is that registration; a function referring to itself is skipped.
func (r *run) references(source []*packages.Package) {
	covered := map[edge]bool{}
	for _, c := range r.result.Calls {
		covered[edge{c.Caller, c.Callee, c.Line}] = true
	}
	for _, p := range source {
		if p.TypesInfo == nil {
			continue
		}
		for _, f := range r.sourceFiles(p) {
			r.fileReferences(p.TypesInfo, f.Syntax, covered)
		}
	}
}

func (r *run) fileReferences(info *types.Info, file *ast.File, covered map[edge]bool) {
	var stack []ast.Node
	ast.Inspect(file, func(n ast.Node) bool {
		if n == nil {
			stack = stack[:len(stack)-1]
			return true
		}
		stack = append(stack, n)
		callee, ok := r.funcValue(info, n, stack)
		if !ok || immediatelyCalled(stack) || r.registered[r.sourcePos(n.Pos())] {
			return true
		}
		caller, ok := entrypoints.EnclosingFunc(stack, r.index)
		if !ok || caller == callee {
			return true
		}
		line := r.fset.PositionFor(n.Pos(), false).Line
		if e := (edge{caller, callee, line}); !covered[e] {
			covered[e] = true
			r.relate(caller, callee, line, protocol.RelationReference, "")
		}
		return true
	})
}

// funcValue returns the function of the tree that n, the last node of
// stack, denotes as a value: a function literal, or an identifier or
// selector naming a function or method.
func (r *run) funcValue(info *types.Info, n ast.Node, stack []ast.Node) (string, bool) {
	var fn *types.Func
	switch e := n.(type) {
	case *ast.FuncLit:
		return r.index.FuncIDAt(e.Type.Func)
	case *ast.Ident:
		if len(stack) >= 2 {
			if sel, ok := stack[len(stack)-2].(*ast.SelectorExpr); ok && sel.Sel == e {
				return "", false // seen as the selector
			}
		}
		fn, _ = info.Uses[e].(*types.Func)
	case *ast.SelectorExpr:
		fn, _ = info.Uses[e.Sel].(*types.Func)
	}
	if fn == nil {
		return "", false
	}
	return r.index.FuncIDAt(fn.Origin().Pos())
}

// immediatelyCalled reports whether the last node of stack is the function
// of a call, possibly parenthesised or instantiated: f(), (f)(), f[int]().
func immediatelyCalled(stack []ast.Node) bool {
	cur := stack[len(stack)-1]
	for i := len(stack) - 2; i >= 0; i-- {
		switch p := stack[i].(type) {
		case *ast.ParenExpr:
			cur = p
			continue
		case *ast.IndexExpr:
			if p.X == cur {
				cur = p
				continue
			}
		case *ast.IndexListExpr:
			if p.X == cur {
				cur = p
				continue
			}
		case *ast.CallExpr:
			return p.Fun == cur
		}
		return false
	}
	return false
}
