package entrypoints

import (
	"go/ast"
	"go/token"
	"go/types"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// dynamicPart stands for a part of a route that is not a constant.
const dynamicPart = "{dynamic}"

// routeShape describes the arguments of one route registration method.
type routeShape struct {
	verb      string // fixed verb for the label, such as "GET"; "" for none
	verbArg   int    // index of a method argument, -1 for none
	pathArg   int    // index of the path or pattern
	handler   int    // index of the (first) handler argument
	variadic  bool   // every argument from handler on is a handler
	serveHTTP bool   // a handler may be a value with a ServeHTTP method
	chained   bool   // the verb may come from a chained .Methods("GET") call
}

// groupShape describes a method that scopes the routes registered through
// the router it returns, or through the router its function argument gets.
type groupShape struct {
	prefixArg int  // index of the path prefix, -1 for none
	fnArg     int  // index of a func(Router) the router runs with the group, -1 for none
	mount     bool // Mount(prefix, handler): the handler serves everything under prefix; returns nothing
}

// routerAPI is one HTTP router recognised by the root package path of its
// module.
type routerAPI struct {
	module      string
	needsMethod bool     // only methods match, not package-level functions
	types       []string // router types of the root package, to recognise router interfaces of the tree
	routes      map[string]routeShape
	groups      map[string]groupShape
	middleware  map[string]int    // method -> index of the first middleware; every argument from there on is one
	handlers    map[string]string // method -> evidence, for methods whose arguments are handlers but not routes
}

func verbs(names []string, fixed func(string) string, shape routeShape) map[string]routeShape {
	m := map[string]routeShape{}
	for _, name := range names {
		s := shape
		s.verb = fixed(name)
		m[name] = s
	}
	return m
}

func merge(maps ...map[string]routeShape) map[string]routeShape {
	out := map[string]routeShape{}
	for _, m := range maps {
		for k, v := range m {
			out[k] = v
		}
	}
	return out
}

var routerAPIs = []routerAPI{
	{
		module: "net/http",
		types:  []string{"ServeMux"},
		routes: map[string]routeShape{
			"HandleFunc": {verbArg: -1, pathArg: 0, handler: 1},
			"Handle":     {verbArg: -1, pathArg: 0, handler: 1, serveHTTP: true},
		},
	},
	{
		module:      "github.com/go-chi/chi",
		needsMethod: true,
		types:       []string{"Mux", "Router"},
		routes: merge(
			verbs([]string{"Get", "Post", "Put", "Patch", "Delete", "Head", "Options", "Connect", "Trace"},
				strings.ToUpper, routeShape{verbArg: -1, pathArg: 0, handler: 1}),
			map[string]routeShape{
				"Handle":     {verbArg: -1, pathArg: 0, handler: 1, serveHTTP: true},
				"HandleFunc": {verbArg: -1, pathArg: 0, handler: 1},
				"Method":     {verbArg: 0, pathArg: 1, handler: 2, serveHTTP: true},
				"MethodFunc": {verbArg: 0, pathArg: 1, handler: 2},
			},
		),
		groups: map[string]groupShape{
			"Route": {prefixArg: 0, fnArg: 1},
			"Group": {prefixArg: -1, fnArg: 0},
			"With":  {prefixArg: -1, fnArg: -1},
			"Mount": {prefixArg: 0, fnArg: -1, mount: true},
		},
		middleware: map[string]int{"Use": 0, "With": 0},
		handlers:   map[string]string{"NotFound": "not found handler", "MethodNotAllowed": "method not allowed handler"},
	},
	{
		module:      "github.com/gin-gonic/gin",
		needsMethod: true,
		types:       []string{"Engine", "RouterGroup"},
		routes: merge(
			verbs([]string{"GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"},
				strings.ToUpper, routeShape{verbArg: -1, pathArg: 0, handler: 1, variadic: true}),
			map[string]routeShape{
				"Any":    {verb: "ANY", verbArg: -1, pathArg: 0, handler: 1, variadic: true},
				"Handle": {verbArg: 0, pathArg: 1, handler: 2, variadic: true},
			},
		),
		groups:     map[string]groupShape{"Group": {prefixArg: 0, fnArg: -1}},
		middleware: map[string]int{"Use": 0, "Group": 1},
		handlers:   map[string]string{"NoRoute": "not found handler", "NoMethod": "method not allowed handler"},
	},
	{
		module:      "github.com/labstack/echo",
		needsMethod: true,
		types:       []string{"Echo", "Group"},
		routes: merge(
			verbs([]string{"GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE"},
				strings.ToUpper, routeShape{verbArg: -1, pathArg: 0, handler: 1}),
			map[string]routeShape{
				"Any": {verb: "ANY", verbArg: -1, pathArg: 0, handler: 1},
				"Add": {verbArg: 0, pathArg: 1, handler: 2},
			},
		),
		groups:     map[string]groupShape{"Group": {prefixArg: 0, fnArg: -1}},
		middleware: map[string]int{"Use": 0, "Pre": 0, "Group": 1},
	},
	{
		module:      "github.com/gorilla/mux",
		needsMethod: true,
		types:       []string{"Router"},
		routes: map[string]routeShape{
			"HandleFunc": {verbArg: -1, pathArg: 0, handler: 1, chained: true},
			"Handle":     {verbArg: -1, pathArg: 0, handler: 1, serveHTTP: true, chained: true},
		},
		groups: map[string]groupShape{
			"PathPrefix": {prefixArg: 0, fnArg: -1},
			"Subrouter":  {prefixArg: -1, fnArg: -1},
		},
		middleware: map[string]int{"Use": 0},
	},
}

// HTTPDetector marks handlers registered on net/http (including Go 1.22
// patterns such as "POST /refunds", kept as the label), chi, gin, echo and
// gorilla/mux, and reports the registrations of routes, middlewares, route
// groups and not-found handlers.
//
// A path is folded from constants; a part that is not constant is shown as
// "{dynamic}". The prefix of a chi Route, Group or Mount, of a gin or echo
// Group and of a gorilla PathPrefix is prepended when the nesting is visible
// in the function that registers the route.
type HTTPDetector struct{}

// Detect implements Detector.
func (HTTPDetector) Detect(pkg *Package, funcs Resolver) []Found {
	d := &httpRun{pkg: pkg, info: pkg.Info, funcs: funcs, scopes: map[ast.Node]*scope{}}
	for _, file := range pkg.Files {
		d.file = file
		var stack []ast.Node
		ast.Inspect(file.Syntax, func(n ast.Node) bool {
			if n == nil {
				stack = stack[:len(stack)-1]
				return true
			}
			stack = append(stack, n)
			if call, ok := n.(*ast.CallExpr); ok {
				d.call(call, stack)
			}
			return true
		})
	}
	return d.found
}

// httpRun is the state of one Detect call.
type httpRun struct {
	pkg    *Package
	info   *types.Info
	funcs  Resolver
	file   File
	scopes map[ast.Node]*scope
	found  []Found

	// The registration call being looked at.
	stack []ast.Node
	line  int
}

func (d *httpRun) call(call *ast.CallExpr, stack []ast.Node) {
	fn := calleeFunc(d.info, call)
	if fn == nil {
		return
	}
	api := d.pkg.Program.routerAPI(fn)
	if api == nil {
		return
	}
	d.stack = stack
	_, d.line = position(d.pkg, d.file, call.Pos())
	name := fn.Name()
	if shape, ok := api.routes[name]; ok {
		d.route(call, shape)
	}
	if g, ok := api.groups[name]; ok {
		d.group(call, g)
	}
	if first, ok := api.middleware[name]; ok {
		d.middleware(call, first)
	}
	if evidence, ok := api.handlers[name]; ok {
		for _, arg := range call.Args {
			d.handlerArg(arg, "ServeHTTP", evidence, protocol.EntryPoint{})
		}
	}
}

func (d *httpRun) route(call *ast.CallExpr, shape routeShape) {
	if len(call.Args) <= shape.handler || len(call.Args) <= shape.pathArg {
		return
	}
	route := d.prefixOf(d.receiver(call), 0) + d.pathLabel(call.Args[shape.pathArg])
	verbs := []string{shape.verb}
	if shape.verbArg >= 0 {
		verb := dynamicPart
		if v, ok := constString(d.info, call.Args[shape.verbArg]); ok {
			verb = strings.ToUpper(v)
		}
		verbs = []string{verb}
	} else if shape.chained {
		if chained := chainedMethods(d.info, call, d.stack); len(chained) > 0 {
			verbs = chained
		}
	}
	handlerArgs := call.Args[shape.handler : shape.handler+1]
	if shape.variadic {
		handlerArgs = call.Args[shape.handler:]
	}
	serve := ""
	if shape.serveHTTP {
		serve = "ServeHTTP"
	}
	regFile, _ := position(d.pkg, d.file, call.Pos())
	for _, arg := range handlerArgs {
		for _, verb := range verbs {
			label := route
			if verb != "" {
				label = verb + " " + route
			}
			d.handlerArg(arg, serve, "route "+label, protocol.EntryPoint{
				Kind: protocol.EntryHTTPRoute, Label: label, File: regFile, Line: d.line,
			})
		}
	}
}

// group handles Route, Group, With, Mount and PathPrefix: a function run
// with the group's router is registered, and a mounted handler serves every
// path under the prefix.
func (d *httpRun) group(call *ast.CallExpr, g groupShape) {
	prefix := d.groupPrefix(call, g, 0)
	if g.mount {
		if len(call.Args) < 2 {
			return
		}
		label := prefix + "/*"
		regFile, _ := position(d.pkg, d.file, call.Pos())
		d.handlerArg(call.Args[1], "ServeHTTP", "route "+label, protocol.EntryPoint{
			Kind: protocol.EntryHTTPRoute, Label: label, File: regFile, Line: d.line,
		})
		return
	}
	if g.fnArg < 0 || g.fnArg >= len(call.Args) {
		return
	}
	evidence := "route group"
	if prefix != "" {
		evidence += " " + prefix
	}
	for _, t := range handlerFuncs(d.info, call.Args[g.fnArg], d.funcs, "") {
		d.register(t, evidence, protocol.EntryPoint{})
	}
}

func (d *httpRun) middleware(call *ast.CallExpr, first int) {
	for i := first; i < len(call.Args); i++ {
		if call.Ellipsis.IsValid() && i == len(call.Args)-1 {
			break // a spread slice: its elements are not visible here
		}
		handlers, wrappers := d.handlerTargets(call.Args[i], "", 0)
		for _, t := range append(handlers, wrappers...) {
			d.register(t, "middleware", protocol.EntryPoint{})
		}
	}
}

// handlerArg registers the functions a handler argument resolves to, with
// the entry point ep unless ep has no kind, and the middlewares wrapped
// around them.
func (d *httpRun) handlerArg(arg ast.Expr, serve, evidence string, ep protocol.EntryPoint) {
	handlers, wrappers := d.handlerTargets(arg, serve, 0)
	for _, t := range handlers {
		d.register(t, evidence, ep)
	}
	for _, t := range wrappers {
		d.register(t, "middleware", protocol.EntryPoint{})
	}
}

// register reports t as registered by the function containing the current
// call, and as the entry point ep when ep has a kind and t is the handler
// itself rather than a constructor of it.
func (d *httpRun) register(t target, evidence string, ep protocol.EntryPoint) {
	f := Found{FuncID: t.id, Registration: registrationAt(d.stack, d.funcs, d.line, evidence, t)}
	if !t.constructor {
		f.EntryPoint = ep
	}
	if f.EntryPoint.Kind == "" && f.Registration == nil {
		return
	}
	d.found = append(d.found, f)
}

// handlerTargets resolves a handler expression to the functions of the tree
// that serve it, and the middlewares wrapped around them:
//
//   - a function value or literal, a conversion of one, or a value with an
//     in-tree serve method (handlerFuncs);
//   - a local variable assigned once in the function, through its value;
//   - a call to a function whose result has the type of one of its
//     parameters, such as mw(next) or http.StripPrefix(p, next): next is the
//     handler, and the closure mw returns (else mw itself) a middleware;
//   - any other call, such as h.Live() or middleware.RequestID(cfg): the
//     closure the called function returns, else the called function itself.
func (d *httpRun) handlerTargets(expr ast.Expr, serve string, depth int) (handlers, wrappers []target) {
	if depth > maxDepth {
		return nil, nil
	}
	expr = ast.Unparen(expr)
	if ts := handlerFuncs(d.info, expr, d.funcs, serve); len(ts) > 0 {
		return ts, nil
	}
	switch e := expr.(type) {
	case *ast.Ident:
		if def, ok := d.definition(e); ok {
			return d.handlerTargets(def, serve, depth+1)
		}
	case *ast.CallExpr:
		if isConversion(d.info, e) {
			return d.handlerTargets(e.Args[0], serve, depth+1)
		}
		fn := calleeFunc(d.info, e)
		if fn == nil {
			return nil, nil
		}
		for _, i := range wrappedArgs(fn) {
			if i < len(e.Args) {
				inner, innerWrappers := d.handlerTargets(e.Args[i], serve, depth+1)
				handlers = append(handlers, inner...)
				wrappers = append(wrappers, innerWrappers...)
			}
		}
		if len(handlers) > 0 {
			return handlers, append(d.constructed(fn), wrappers...)
		}
		return d.constructed(fn), nil
	}
	return nil, nil
}

// wrappedArgs returns the indexes of the parameters of fn that have the type
// of its single result: the handler a wrapper such as mw(next) wraps.
func wrappedArgs(fn *types.Func) []int {
	sig := fn.Signature()
	if sig.Results().Len() != 1 {
		return nil
	}
	result := sig.Results().At(0).Type()
	var out []int
	for i := range sig.Params().Len() {
		if sig.Variadic() && i == sig.Params().Len()-1 {
			break
		}
		if types.Identical(sig.Params().At(i).Type(), result) {
			out = append(out, i)
		}
	}
	return out
}

// constructed returns what a call to fn registers: the functions of the tree
// fn returns, or fn itself when they cannot be resolved. A function outside
// the tree registers nothing.
func (d *httpRun) constructed(fn *types.Func) []target {
	id, ok := d.funcs.FuncIDAt(fn.Origin().Pos())
	if !ok {
		return nil
	}
	if ids := d.pkg.Program.Returned(id); len(ids) > 0 {
		out := make([]target, 0, len(ids))
		for _, r := range ids {
			out = append(out, target{id: r})
		}
		return out
	}
	return []target{{id: id, constructor: true}}
}

// receiver returns the receiver expression of a method call, or nil.
func (d *httpRun) receiver(call *ast.CallExpr) ast.Expr {
	sel, ok := ast.Unparen(call.Fun).(*ast.SelectorExpr)
	if !ok || d.info == nil {
		return nil
	}
	if _, ok := d.info.Selections[sel]; !ok {
		return nil // a qualified identifier such as http.HandleFunc
	}
	return sel.X
}

// pathLabel folds a path expression: a constant, or a concatenation whose
// parts are constants or shown as "{dynamic}".
func (d *httpRun) pathLabel(expr ast.Expr) string {
	if s, ok := constString(d.info, expr); ok {
		return s
	}
	if b, ok := ast.Unparen(expr).(*ast.BinaryExpr); ok && b.Op == token.ADD {
		label := d.pathLabel(b.X) + d.pathLabel(b.Y)
		return strings.ReplaceAll(label, dynamicPart+dynamicPart, dynamicPart)
	}
	return dynamicPart
}

// prefixOf returns the path prefix of the routes registered on the router
// expr, as far as the function that uses it shows: "" when it has none or
// it is not known.
func (d *httpRun) prefixOf(expr ast.Expr, depth int) string {
	if expr == nil || depth > maxDepth {
		return ""
	}
	switch e := ast.Unparen(expr).(type) {
	case *ast.CallExpr:
		fn := calleeFunc(d.info, e)
		if fn == nil {
			return ""
		}
		api := d.pkg.Program.routerAPI(fn)
		if api == nil {
			return ""
		}
		g, ok := api.groups[fn.Name()]
		if !ok || g.mount {
			return ""
		}
		return d.groupPrefix(e, g, depth+1)
	case *ast.Ident:
		obj := d.localVar(e)
		if obj == nil {
			return ""
		}
		s := d.scope()
		if use, ok := s.groups[obj]; ok {
			return d.groupPrefix(use.call, use.shape, depth+1)
		}
		prefix := ""
		if def, ok := s.single(obj); ok {
			prefix = d.prefixOf(def, depth+1)
		}
		if use, ok := s.mounts[obj]; ok {
			prefix = d.groupPrefix(use.call, use.shape, depth+1) + prefix
		}
		return prefix
	}
	return ""
}

// groupPrefix returns the prefix of the routes registered inside the group
// call: the prefix of its router plus its own.
func (d *httpRun) groupPrefix(call *ast.CallExpr, g groupShape, depth int) string {
	prefix := d.prefixOf(d.receiver(call), depth)
	if g.prefixArg >= 0 && g.prefixArg < len(call.Args) {
		prefix += d.pathLabel(call.Args[g.prefixArg])
	}
	return prefix
}

// definition returns the single value a local variable is assigned in the
// declaration being looked at.
func (d *httpRun) definition(id *ast.Ident) (ast.Expr, bool) {
	obj := d.localVar(id)
	if obj == nil {
		return nil, false
	}
	return d.scope().single(obj)
}

// localVar returns the local variable id refers to, or nil.
func (d *httpRun) localVar(id *ast.Ident) *types.Var {
	if d.info == nil {
		return nil
	}
	v, ok := d.info.Uses[id].(*types.Var)
	if !ok || v.IsField() || v.Pkg() == nil || v.Parent() == nil || v.Parent() == v.Pkg().Scope() {
		return nil
	}
	return v
}

// chainedMethods returns the literal verbs of a gorilla/mux style
// r.HandleFunc(path, h).Methods("GET", "POST") chain around call.
func chainedMethods(info *types.Info, call *ast.CallExpr, stack []ast.Node) []string {
	if len(stack) < 3 {
		return nil
	}
	sel, ok := stack[len(stack)-2].(*ast.SelectorExpr)
	if !ok || sel.X != call || sel.Sel.Name != "Methods" {
		return nil
	}
	outer, ok := stack[len(stack)-3].(*ast.CallExpr)
	if !ok || outer.Fun != sel {
		return nil
	}
	var verbs []string
	for _, arg := range outer.Args {
		v, ok := constString(info, arg)
		if !ok {
			return nil
		}
		verbs = append(verbs, strings.ToUpper(v))
	}
	return verbs
}
