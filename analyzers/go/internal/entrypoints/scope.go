package entrypoints

import (
	"go/ast"
	"go/token"
	"go/types"
)

// scope is what one top-level declaration shows about its local variables:
// the values they are assigned, the group a router parameter belongs to, and
// the Mount call that attaches a router under a prefix.
type scope struct {
	defs   map[*types.Var][]ast.Expr // a nil value: assigned something that is not one expression
	groups map[*types.Var]groupUse   // parameter of a func(Router) run by a group call
	mounts map[*types.Var]groupUse   // router mounted by a Mount call (the first one)
}

// groupUse is a group or Mount call.
type groupUse struct {
	call  *ast.CallExpr
	shape groupShape
}

// single returns the value v is assigned when it is assigned exactly once,
// from one expression.
func (s *scope) single(v *types.Var) (ast.Expr, bool) {
	defs := s.defs[v]
	if len(defs) != 1 || defs[0] == nil {
		return nil, false
	}
	return defs[0], true
}

// scope returns the scope of the top-level declaration containing the
// current call.
func (d *httpRun) scope() *scope {
	if len(d.stack) < 2 {
		return &scope{}
	}
	decl := d.stack[1] // stack[0] is the file
	if s, ok := d.scopes[decl]; ok {
		return s
	}
	s := &scope{defs: map[*types.Var][]ast.Expr{}, groups: map[*types.Var]groupUse{}, mounts: map[*types.Var]groupUse{}}
	d.scopes[decl] = s
	if d.info == nil {
		return s
	}
	ast.Inspect(decl, func(n ast.Node) bool {
		switch n := n.(type) {
		case *ast.AssignStmt:
			pairs := len(n.Lhs) == len(n.Rhs) && (n.Tok == token.DEFINE || n.Tok == token.ASSIGN)
			for i, lhs := range n.Lhs {
				var value ast.Expr
				if pairs {
					value = n.Rhs[i]
				}
				d.addDef(s, lhs, value)
			}
		case *ast.ValueSpec:
			for i, name := range n.Names {
				if len(n.Values) == 0 {
					break // the zero value does not count as an assignment
				}
				var value ast.Expr
				if len(n.Names) == len(n.Values) {
					value = n.Values[i]
				}
				d.addDef(s, name, value)
			}
		case *ast.RangeStmt:
			if n.Tok != token.ILLEGAL {
				d.addDef(s, n.Key, nil)
				d.addDef(s, n.Value, nil)
			}
		case *ast.UnaryExpr:
			if n.Op == token.AND {
				d.addDef(s, n.X, nil) // may be assigned through the pointer
			}
		case *ast.CallExpr:
			d.addGroupUse(s, n)
		}
		return true
	})
	return s
}

func (d *httpRun) addDef(s *scope, lhs ast.Expr, value ast.Expr) {
	if lhs == nil {
		return
	}
	id, ok := ast.Unparen(lhs).(*ast.Ident)
	if !ok {
		return
	}
	obj := d.info.Defs[id]
	if obj == nil {
		obj = d.info.Uses[id]
	}
	if v, ok := obj.(*types.Var); ok {
		s.defs[v] = append(s.defs[v], value)
	}
}

// addGroupUse records the router parameter of a func(Router) given to a
// group call, and the router mounted by a Mount call.
func (d *httpRun) addGroupUse(s *scope, call *ast.CallExpr) {
	fn := calleeFunc(d.info, call)
	if fn == nil {
		return
	}
	api := d.pkg.Program.routerAPI(fn)
	if api == nil {
		return
	}
	g, ok := api.groups[fn.Name()]
	if !ok {
		return
	}
	use := groupUse{call: call, shape: g}
	if g.fnArg >= 0 && g.fnArg < len(call.Args) {
		lit, ok := ast.Unparen(call.Args[g.fnArg]).(*ast.FuncLit)
		if ok && lit.Type.Params != nil && len(lit.Type.Params.List) > 0 && len(lit.Type.Params.List[0].Names) > 0 {
			if v, ok := d.info.Defs[lit.Type.Params.List[0].Names[0]].(*types.Var); ok {
				s.groups[v] = use
			}
		}
	}
	if g.mount && len(call.Args) >= 2 {
		if id, ok := ast.Unparen(call.Args[1]).(*ast.Ident); ok {
			if v := d.localVar(id); v != nil {
				if _, seen := s.mounts[v]; !seen {
					s.mounts[v] = use
				}
			}
		}
	}
}
