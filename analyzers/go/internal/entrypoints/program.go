package entrypoints

import (
	"go/ast"
	"go/types"
	"slices"
)

// maxDepth bounds every chain the detectors follow (variables, prefixes,
// wrappers, returned functions), so that cycles end.
const maxDepth = 8

// Program is what detectors know about the whole load, beyond one package:
// the router types of the frameworks it imports, and the functions each
// function of the tree returns as values.
type Program struct {
	routers  [][]types.Type // per entry of routerAPIs
	returns  map[string]*returned
	ifaceAPI map[types.Type]int // router interface of the tree -> index in routerAPIs, -1 for none
}

// returned is what one function returns: functions of the tree as values,
// and the results of calls to other functions of the tree.
type returned struct {
	funcs []string
	via   []string
}

// NewProgram collects what the detectors need from every package of a load.
func NewProgram(pkgs []*Package, funcs Resolver) *Program {
	p := &Program{
		routers:  make([][]types.Type, len(routerAPIs)),
		returns:  map[string]*returned{},
		ifaceAPI: map[types.Type]int{},
	}
	seen := map[*types.Package]bool{}
	for _, pkg := range pkgs {
		if pkg.Types != nil {
			p.addRouterTypes(pkg.Types, seen)
			for _, imp := range pkg.Types.Imports() {
				p.addRouterTypes(imp, seen)
			}
		}
		if pkg.Info == nil {
			continue
		}
		for _, f := range pkg.Files {
			p.collectReturns(pkg.Info, f.Syntax, funcs)
		}
	}
	return p
}

// addRouterTypes records the router types tp declares when it is the root
// package of a recognised router.
func (p *Program) addRouterTypes(tp *types.Package, seen map[*types.Package]bool) {
	if seen[tp] {
		return
	}
	seen[tp] = true
	for i, api := range routerAPIs {
		if !isModuleRoot(tp.Path(), api.module) {
			continue
		}
		for _, name := range api.types {
			obj, ok := tp.Scope().Lookup(name).(*types.TypeName)
			if !ok {
				continue
			}
			t := obj.Type()
			if !types.IsInterface(t) {
				t = types.NewPointer(t)
			}
			p.routers[i] = append(p.routers[i], t)
		}
	}
}

// routerAPI returns the router fn belongs to: a function or method of a
// recognised router package, or a method of an interface declared elsewhere
// (in the tree, typically to avoid importing the router) that one of the
// router's types implements. It returns nil for anything else.
func (p *Program) routerAPI(fn *types.Func) *routerAPI {
	path := pkgPathOf(fn)
	recv := fn.Signature().Recv()
	for i := range routerAPIs {
		api := &routerAPIs[i]
		if isModuleRoot(path, api.module) {
			if api.needsMethod && recv == nil {
				return nil
			}
			return api
		}
	}
	if p == nil || recv == nil || !types.IsInterface(recv.Type()) {
		return nil
	}
	return p.implementedBy(recv.Type())
}

func (p *Program) implementedBy(t types.Type) *routerAPI {
	idx, ok := p.ifaceAPI[t]
	if !ok {
		idx = -1
		iface, _ := t.Underlying().(*types.Interface)
		if iface != nil && iface.NumMethods() > 0 {
		search:
			for i, routers := range p.routers {
				for _, r := range routers {
					if types.Implements(r, iface) {
						idx = i
						break search
					}
				}
			}
		}
		p.ifaceAPI[t] = idx
	}
	if idx < 0 {
		return nil
	}
	return &routerAPIs[idx]
}

// Returned returns the functions of the tree that function id returns as
// values, directly or through the results of other functions it returns,
// such as the middleware closure returned by a constructor.
func (p *Program) Returned(id string) []string {
	if p == nil {
		return nil
	}
	var out []string
	seen := map[string]bool{}
	var walk func(id string, depth int)
	walk = func(id string, depth int) {
		if seen[id] || depth > maxDepth {
			return
		}
		seen[id] = true
		r := p.returns[id]
		if r == nil {
			return
		}
		for _, f := range r.funcs {
			if !slices.Contains(out, f) {
				out = append(out, f)
			}
		}
		for _, v := range r.via {
			walk(v, depth+1)
		}
	}
	walk(id, 0)
	return out
}

func (p *Program) collectReturns(info *types.Info, file *ast.File, funcs Resolver) {
	for _, decl := range file.Decls {
		fd, ok := decl.(*ast.FuncDecl)
		if !ok || fd.Body == nil {
			continue
		}
		if id, ok := funcs.FuncIDAt(fd.Name.Pos()); ok {
			p.bodyReturns(info, id, fd.Body, funcs)
		}
	}
}

// bodyReturns records the return statements of body, which belongs to the
// function owner, and recursively those of the closures inside it.
func (p *Program) bodyReturns(info *types.Info, owner string, body *ast.BlockStmt, funcs Resolver) {
	ast.Inspect(body, func(n ast.Node) bool {
		switch n := n.(type) {
		case *ast.FuncLit:
			if id, ok := funcs.FuncIDAt(n.Type.Func); ok {
				p.bodyReturns(info, id, n.Body, funcs)
			}
			return false
		case *ast.ReturnStmt:
			for _, res := range n.Results {
				p.addReturn(info, owner, res, funcs)
			}
		}
		return true
	})
}

func (p *Program) addReturn(info *types.Info, owner string, expr ast.Expr, funcs Resolver) {
	r := p.returns[owner]
	if r == nil {
		r = &returned{}
		p.returns[owner] = r
	}
	if ts := handlerFuncs(info, expr, funcs, ""); len(ts) > 0 {
		for _, t := range ts {
			if !slices.Contains(r.funcs, t.id) {
				r.funcs = append(r.funcs, t.id)
			}
		}
		return
	}
	call, ok := ast.Unparen(expr).(*ast.CallExpr)
	if !ok || isConversion(info, call) {
		return
	}
	if fn := calleeFunc(info, call); fn != nil {
		if id, ok := funcs.FuncIDAt(fn.Origin().Pos()); ok && id != owner && !slices.Contains(r.via, id) {
			r.via = append(r.via, id)
		}
	}
}
