// Package analyzer turns loaded Go packages into the protocol's functions,
// calls, externals and diagnostics.
package analyzer

import (
	"fmt"
	"go/token"
	"go/types"
	"slices"
	"strconv"
	"strings"
	"time"

	"golang.org/x/tools/go/callgraph"
	"golang.org/x/tools/go/callgraph/cha"
	"golang.org/x/tools/go/callgraph/vta"
	"golang.org/x/tools/go/packages"
	"golang.org/x/tools/go/ssa"
	"golang.org/x/tools/go/ssa/ssautil"

	"github.com/rootless-dev/difflens/analyzers/go/internal/entrypoints"
	"github.com/rootless-dev/difflens/analyzers/go/internal/funcindex"
	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// Result is what one load contributes to the output document.
type Result struct {
	Functions   []protocol.Function
	Calls       []protocol.Call
	Externals   []protocol.External
	Diagnostics []protocol.Diagnostic
}

// Append adds other's lists to r.
func (r *Result) Append(other Result) {
	r.Functions = append(r.Functions, other.Functions...)
	r.Calls = append(r.Calls, other.Calls...)
	r.Externals = append(r.Externals, other.Externals...)
	r.Diagnostics = append(r.Diagnostics, other.Diagnostics...)
}

// Analyzer analyses the packages of one load.
type Analyzer struct {
	log       logging.Logger
	detectors []entrypoints.Detector
}

// New returns an analyzer that reports progress to log and finds entry points
// with detectors.
func New(log logging.Logger, detectors []entrypoints.Detector) *Analyzer {
	return &Analyzer{log: log, detectors: detectors}
}

// run holds the state of one Analyze call.
type run struct {
	log    logging.Logger
	tree   Tree
	fset   *token.FileSet
	index  *funcindex.Index
	result Result

	externals  map[string]protocol.External
	dynamic    map[ssa.CallInstruction][]*ssa.Function // call graph targets
	fallback   map[ssa.CallInstruction][]*ssa.Function // CHA targets, used when the call graph has none
	registered map[sourcePos]bool                      // function values handed over by a registration
}

// sourcePos is a position compared by file and offset, so that a file parsed
// twice (in a package and in its test variant) gives the same position.
type sourcePos struct {
	file   string
	offset int
}

func (r *run) sourcePos(pos token.Pos) sourcePos {
	p := r.fset.PositionFor(pos, false)
	return sourcePos{file: p.Filename, offset: p.Offset}
}

// Analyze analyses pkgs, the result of loading "./..." with tests in tree.
// It never fails: problems become diagnostics and the analysis continues with
// whatever could be loaded.
func (a *Analyzer) Analyze(tree Tree, pkgs []*packages.Package, fset *token.FileSet) Result {
	r := &run{
		log:        a.log,
		tree:       tree,
		fset:       fset,
		index:      funcindex.New(fset),
		externals:  map[string]protocol.External{},
		registered: map[sourcePos]bool{},
	}
	r.packageDiagnostics(pkgs)

	source := sourcePackages(pkgs)
	for _, p := range source {
		r.index.Add(r.indexPackage(p))
	}
	entries := r.entryPoints(source, a.detectors)

	start := time.Now()
	prog, built := r.buildSSA(source)
	all := ssautil.AllFunctions(prog)
	funcs := sourceFunctions(prog, built, all)
	r.callGraph(prog, all, funcs)
	r.log.Debug("call graph built", "functions", len(funcs), "elapsed", time.Since(start).String())
	for _, fn := range funcs {
		r.functionCalls(fn)
	}
	r.references(source)

	for _, f := range r.index.Funcs() {
		r.result.Functions = append(r.result.Functions, protocol.Function{
			ID:            f.ID,
			Name:          f.Name,
			QualifiedName: f.QualifiedName,
			Container:     f.Container,
			File:          f.File,
			StartLine:     f.StartLine,
			EndLine:       f.EndLine,
			Generated:     f.Generated,
			Test:          f.IsTest(),
			Complexity:    f.Complexity,
			EntryPoints:   entries[f.ID],
		})
	}
	for _, ext := range r.externals {
		r.result.Externals = append(r.result.Externals, ext)
	}
	return r.result
}

// sourcePackages returns the packages whose files belong to the tree, sorted
// by id, without the generated test main packages.
func sourcePackages(pkgs []*packages.Package) []*packages.Package {
	var out []*packages.Package
	for _, p := range pkgs {
		if p.Name == "main" && strings.HasSuffix(p.ID, ".test") {
			continue // synthesized "go test" main package
		}
		out = append(out, p)
	}
	slices.SortFunc(out, func(a, b *packages.Package) int { return strings.Compare(a.ID, b.ID) })
	return out
}

func (r *run) indexPackage(p *packages.Package) funcindex.Package {
	pkg := funcindex.Package{Path: p.PkgPath, Name: p.Name}
	for _, f := range r.sourceFiles(p) {
		pkg.Files = append(pkg.Files, funcindex.File{Syntax: f.Syntax, RelPath: f.RelPath})
	}
	firstFile := ""
	if len(pkg.Files) > 0 {
		firstFile = pkg.Files[0].RelPath
	}
	pkg.Container = funcindex.Container(p.Name, firstFile)
	return pkg
}

// sourceFiles returns the parsed files of p that lie inside the tree.
func (r *run) sourceFiles(p *packages.Package) []entrypoints.File {
	var files []entrypoints.File
	for _, f := range p.Syntax {
		name := r.fset.PositionFor(f.Pos(), false).Filename
		rel, ok := r.tree.Rel(name)
		if !ok {
			continue
		}
		files = append(files, entrypoints.File{Syntax: f, RelPath: rel, IsTest: strings.HasSuffix(rel, "_test.go")})
	}
	return files
}

// entryPoints runs the detectors, returns the entry points of each function
// id and records the registrations they found as calls.
func (r *run) entryPoints(source []*packages.Package, detectors []entrypoints.Detector) map[string][]protocol.EntryPoint {
	var pkgs []*entrypoints.Package
	for _, p := range source {
		if p.TypesInfo == nil {
			continue
		}
		pkgs = append(pkgs, &entrypoints.Package{
			Path: p.PkgPath, Name: p.Name, Fset: r.fset, Files: r.sourceFiles(p), Info: p.TypesInfo, Types: p.Types,
		})
	}
	program := entrypoints.NewProgram(pkgs, r.index)
	entries := map[string][]protocol.EntryPoint{}
	for _, pkg := range pkgs {
		pkg.Program = program
		for _, d := range detectors {
			for _, found := range d.Detect(pkg, r.index) {
				if found.EntryPoint.Kind != "" && !slices.Contains(entries[found.FuncID], found.EntryPoint) {
					entries[found.FuncID] = append(entries[found.FuncID], found.EntryPoint)
				}
				if reg := found.Registration; reg != nil {
					r.relate(reg.Caller, found.FuncID, reg.Line, protocol.RelationRegistration, reg.Evidence)
					if reg.Value.IsValid() {
						r.registered[r.sourcePos(reg.Value)] = true
					}
				}
			}
		}
	}
	return entries
}

// relate records a registration or a reference, which are always async and
// static.
func (r *run) relate(caller, callee string, line int, relation, evidence string) {
	r.result.Calls = append(r.result.Calls, protocol.Call{
		Caller: caller, Callee: callee, Line: line,
		Kind: protocol.KindAsync, Resolution: protocol.ResolutionStatic,
		Relation: relation, Evidence: evidence,
	})
}

// wellTyped reports whether SSA can be built from p's syntax.
func wellTyped(p *packages.Package) bool {
	return p.Types != nil && p.TypesInfo != nil && len(p.Errors) == 0 && len(p.TypeErrors) == 0
}

// buildSSA creates an SSA program in which the packages of the tree have
// bodies and their direct dependencies are created from type information
// only. Packages with errors are created from types only, so that packages
// importing them still build; their own calls are not analysed.
func (r *run) buildSSA(source []*packages.Package) (*ssa.Program, []*ssa.Package) {
	// Generic functions are also built per instantiation, so that calls of
	// function values and type-parameter methods inside them resolve.
	prog := ssa.NewProgram(r.fset, ssa.InstantiateGenerics)
	isSource := map[*types.Package]bool{}
	for _, p := range source {
		if p.Types != nil {
			isSource[p.Types] = true
		}
	}
	created := map[*types.Package]bool{}
	create := func(tp *types.Package, p *packages.Package) *ssa.Package {
		if created[tp] {
			return nil
		}
		created[tp] = true
		var pkg *ssa.Package
		r.protect("create SSA for "+tp.Path(), func() {
			if p != nil {
				pkg = prog.CreatePackage(tp, p.Syntax, p.TypesInfo, true)
			} else {
				pkg = prog.CreatePackage(tp, nil, nil, true)
			}
		})
		return pkg
	}
	var withBodies []*ssa.Package
	for _, p := range source {
		if p.Types == nil {
			continue
		}
		if wellTyped(p) {
			if pkg := create(p.Types, p); pkg != nil {
				withBodies = append(withBodies, pkg)
			}
			continue
		}
		create(p.Types, nil)
		if len(r.sourceFiles(p)) > 0 {
			r.diag(protocol.LevelWarning, fmt.Sprintf("calls of package %s are not analysed because it has errors", p.ID), "", 0)
		}
	}
	for _, p := range source {
		if p.Types == nil {
			continue
		}
		for _, imp := range p.Types.Imports() {
			if !isSource[imp] {
				create(imp, nil)
			}
		}
	}
	var built []*ssa.Package
	for _, pkg := range withBodies {
		ok := r.protect("build SSA for "+pkg.Pkg.Path(), pkg.Build)
		if ok {
			built = append(built, pkg)
		}
	}
	return prog, built
}

// protect runs f, turning a panic into a diagnostic. It reports whether f
// completed.
func (r *run) protect(what string, f func()) (ok bool) {
	defer func() {
		if v := recover(); v != nil {
			r.diag(protocol.LevelWarning, fmt.Sprintf("%s failed: %v", what, v), "", 0)
			ok = false
		}
	}()
	f()
	return true
}

// sourceFunctions returns every function with a body in pkgs: package-level
// functions, methods (generic ones included), init functions, recursively the
// closures inside them, and the instantiations (found in all) of the generic
// ones.
func sourceFunctions(prog *ssa.Program, pkgs []*ssa.Package, all map[*ssa.Function]bool) []*ssa.Function {
	seen := map[*ssa.Function]bool{}
	var out []*ssa.Function
	var add func(fn *ssa.Function)
	add = func(fn *ssa.Function) {
		if fn == nil || seen[fn] {
			return
		}
		seen[fn] = true
		out = append(out, fn)
		for _, anon := range fn.AnonFuncs {
			add(anon)
		}
	}
	for _, pkg := range pkgs {
		for _, mem := range pkg.Members {
			switch mem := mem.(type) {
			case *ssa.Function:
				add(mem)
			case *ssa.Type:
				if named, ok := mem.Type().(*types.Named); ok {
					for i := range named.NumMethods() {
						add(prog.FuncValue(named.Method(i)))
					}
				}
			}
		}
		// User init functions are not members; the package initializer calls them.
		if init := pkg.Func("init"); init != nil {
			for _, b := range init.Blocks {
				for _, instr := range b.Instrs {
					if call, ok := instr.(*ssa.Call); ok {
						if callee := call.Call.StaticCallee(); callee != nil && callee.Pkg == pkg {
							add(callee)
						}
					}
				}
			}
		}
	}
	var instances []*ssa.Function
	for fn := range all {
		if origin := fn.Origin(); origin != nil && seen[origin] {
			instances = append(instances, fn)
		}
	}
	slices.SortFunc(instances, func(a, b *ssa.Function) int { return strings.Compare(a.String(), b.String()) })
	for _, fn := range instances {
		add(fn)
	}
	return out
}

// callGraph resolves dynamic calls. VTA (variable type analysis) is used
// because it only reports the targets whose values actually flow to a call
// site, where CHA would list every type in the program that implements the
// interface (every io.Writer of the standard library). CHA is kept as the
// initial graph VTA requires and as a fallback for interface calls VTA
// cannot resolve (values that come from dependencies, whose bodies are not
// loaded); the fallback only ever contributes targets inside the tree.
func (r *run) callGraph(prog *ssa.Program, all map[*ssa.Function]bool, funcs []*ssa.Function) {
	r.dynamic = map[ssa.CallInstruction][]*ssa.Function{}
	r.fallback = map[ssa.CallInstruction][]*ssa.Function{}
	var chaGraph, vtaGraph *callgraph.Graph
	if !r.protect("CHA call graph", func() { chaGraph = cha.CallGraph(prog) }) {
		return
	}
	for _, fn := range funcs {
		all[fn] = true
	}
	if !r.protect("VTA call graph", func() { vtaGraph = vta.CallGraph(all, chaGraph) }) {
		vtaGraph = chaGraph
	}
	collect := func(g *callgraph.Graph, into map[ssa.CallInstruction][]*ssa.Function) {
		for _, fn := range funcs {
			node := g.Nodes[fn]
			if node == nil {
				continue
			}
			for _, edge := range node.Out {
				if edge.Site != nil && edge.Site.Common().StaticCallee() == nil {
					into[edge.Site] = append(into[edge.Site], edge.Callee.Func)
				}
			}
		}
	}
	collect(vtaGraph, r.dynamic)
	collect(chaGraph, r.fallback)
}

// callerOf returns the indexed function fn belongs to. Synthetic functions
// with a parent (the bodies of range-over-func loops) belong to their
// enclosing function; viaSynthetic is then true.
func (r *run) callerOf(fn *ssa.Function) (f *funcindex.Func, viaSynthetic bool, ok bool) {
	for cur := fn; cur != nil; cur = cur.Parent() {
		if cur.Synthetic == "" || cur.Origin() != nil {
			// A generic instantiation shares its origin's position.
			if f, ok := r.index.At(cur.Pos()); ok {
				return f, cur != fn, true
			}
		}
		if cur.Parent() == nil {
			break
		}
	}
	return nil, false, false
}

func (r *run) functionCalls(fn *ssa.Function) {
	if len(fn.Blocks) == 0 || (fn.Synthetic != "" && fn.Parent() == nil && fn.Origin() == nil) {
		return // no body, or a wrapper (bound method, thunk, promoted method)
	}
	caller, viaSynthetic, ok := r.callerOf(fn)
	if !ok {
		return
	}
	kinds := newBlockKinds(fn)
	for _, b := range fn.Blocks {
		for _, instr := range b.Instrs {
			site, ok := instr.(ssa.CallInstruction)
			if !ok {
				continue
			}
			if _, builtin := site.Common().Value.(*ssa.Builtin); builtin {
				continue
			}
			kind := protocol.KindConditional
			switch {
			case isGo(site):
				kind = protocol.KindAsync
			case !viaSynthetic && kinds.always(b):
				kind = protocol.KindAlways
			}
			line := caller.StartLine
			if pos := site.Pos(); pos.IsValid() {
				line = r.fset.PositionFor(pos, false).Line
			}
			r.site(caller, site, line, kind)
		}
	}
}

func isGo(site ssa.CallInstruction) bool {
	_, ok := site.(*ssa.Go)
	return ok
}

func (r *run) site(callerFunc *funcindex.Func, site ssa.CallInstruction, line int, kind string) {
	caller := callerFunc.ID
	common := site.Common()
	if callee := common.StaticCallee(); callee != nil {
		if t, ok := r.resolve(callee); ok {
			r.emit(caller, t, line, kind, protocol.ResolutionStatic)
			r.asyncArguments(caller, t.id, callee, common, line)
		}
		return
	}

	if common.IsInvoke() && common.Method.Pkg() == nil {
		// error.Error: error values flow almost everywhere, so the call
		// graph would list every error type of the tree. Like interfaces
		// of dependencies, it is summarised by the interface method.
		r.emit(caller, externalTarget(common.Method), line, kind, protocol.ResolutionDynamic)
		return
	}
	targets := r.dynamic[site]
	if len(targets) == 0 && common.IsInvoke() {
		targets = r.fallback[site]
	}
	var resolved []target
	outside := false
	for _, fn := range targets {
		t, ok := r.resolve(fn)
		if !ok {
			continue
		}
		if t.ext != nil && common.IsInvoke() {
			outside = true
			continue
		}
		// Tests are loaded with the code they test, so stubs and fakes
		// they assign flow into production call sites; a production
		// caller does not list targets declared in _test.go files.
		if f, ok := r.index.ByID(t.id); ok && f.IsTest() && !callerFunc.IsTest() {
			continue
		}
		resolved = append(resolved, t)
	}
	// Interface calls whose targets lie outside the tree, or that have no
	// known target at all, are summarised by the interface method itself
	// instead of listing every implementation found in dependencies.
	if common.IsInvoke() && (outside || len(resolved) == 0) {
		resolved = append(resolved, externalTarget(common.Method))
	}
	slices.SortFunc(resolved, func(a, b target) int { return strings.Compare(a.id, b.id) })
	resolved = slices.CompactFunc(resolved, func(a, b target) bool { return a.id == b.id })
	for _, t := range resolved {
		r.emit(caller, t, line, kind, protocol.ResolutionDynamic)
	}
}

// target is a resolved callee: a function of the tree (ext == nil) or an
// external.
type target struct {
	id  string
	ext *protocol.External
}

// resolve returns the function or external fn denotes. Synthetic wrappers
// (bound method values, thunks, promoted methods) resolve to the method they
// wrap.
func (r *run) resolve(fn *ssa.Function) (target, bool) {
	if fn.Origin() != nil {
		fn = fn.Origin()
	}
	if fn.Synthetic != "" {
		if fn.Parent() != nil {
			return target{}, false // range-over-func body, not a callable function
		}
		obj, ok := fn.Object().(*types.Func)
		if !ok {
			return target{}, false
		}
		obj = obj.Origin()
		if id, ok := r.index.FuncIDAt(obj.Pos()); ok {
			return target{id: id}, true
		}
		return externalTarget(obj), true
	}
	if id, ok := r.index.FuncIDAt(fn.Pos()); ok {
		return target{id: id}, true
	}
	if obj, ok := fn.Object().(*types.Func); ok {
		return externalTarget(obj.Origin()), true
	}
	if fn.Pkg != nil && fn.Parent() == nil {
		p := fn.Pkg.Pkg.Path()
		return target{id: p + "." + fn.Name(), ext: &protocol.External{ID: p + "." + fn.Name(), Name: fn.Name(), Container: p}}, true
	}
	return target{}, false
}

// externalTarget returns fn as an external whose id is written like function
// ids: "fmt.Println", "net/http.(*ServeMux).Handle", "io.Writer.Write" (an
// interface method).
func externalTarget(fn *types.Func) target {
	container := "builtin"
	if fn.Pkg() != nil {
		container = fn.Pkg().Path()
	}
	id, name := container+"."+fn.Name(), fn.Name()
	if recv := fn.Signature().Recv(); recv != nil {
		typeName, pointer := receiverTypeName(recv.Type())
		name = typeName + "." + fn.Name()
		id = container + "." + name
		if pointer {
			id = container + ".(*" + typeName + ")." + fn.Name()
		}
	}
	return target{id: id, ext: &protocol.External{ID: id, Name: name, Container: container}}
}

func receiverTypeName(t types.Type) (name string, pointer bool) {
	if p, ok := t.(*types.Pointer); ok {
		pointer = true
		t = p.Elem()
	}
	switch t := types.Unalias(t).(type) {
	case *types.Named:
		return t.Obj().Name(), pointer
	case *types.TypeParam:
		return t.Obj().Name(), pointer
	default:
		return "interface", pointer
	}
}

// emit records a call, and its callee as an external when it is one.
func (r *run) emit(caller string, t target, line int, kind, resolution string) {
	if t.ext != nil {
		r.externals[t.id] = *t.ext
	}
	r.result.Calls = append(r.result.Calls, protocol.Call{
		Caller: caller, Callee: t.id, Line: line, Kind: kind, Resolution: resolution,
		Relation: protocol.RelationCall,
	})
}

// asyncAPIs lists functions that run a function argument later or on another
// goroutine: external id and index of the function argument.
var asyncAPIs = map[string]int{
	"time.AfterFunc":                            1,
	"context.AfterFunc":                         1,
	"sync.(*WaitGroup).Go":                      0,
	"golang.org/x/sync/errgroup.(*Group).Go":    0,
	"golang.org/x/sync/errgroup.(*Group).TryGo": 0,
}

// asyncArguments adds an async call to the function handed to a known async
// API, such as the f in time.AfterFunc(d, f).
func (r *run) asyncArguments(caller, calleeID string, callee *ssa.Function, common *ssa.CallCommon, line int) {
	idx, ok := asyncAPIs[calleeID]
	if !ok {
		return
	}
	if callee.Signature.Recv() != nil {
		idx++ // the receiver is the first argument of a static method call
	}
	if idx >= len(common.Args) {
		return
	}
	var fn *ssa.Function
	switch v := common.Args[idx].(type) {
	case *ssa.Function:
		fn = v
	case *ssa.MakeClosure:
		fn, _ = v.Fn.(*ssa.Function)
	}
	if fn == nil {
		return
	}
	if t, ok := r.resolve(fn); ok {
		r.emit(caller, t, line, protocol.KindAsync, protocol.ResolutionStatic)
	}
}

// packageDiagnostics turns load and type errors into diagnostics.
func (r *run) packageDiagnostics(pkgs []*packages.Package) {
	packages.Visit(pkgs, nil, func(p *packages.Package) {
		typeErrors := slices.ContainsFunc(p.Errors, func(e packages.Error) bool {
			return e.Kind == packages.TypeError || e.Kind == packages.ParseError
		})
		for _, e := range p.Errors {
			if typeErrors && e.Kind == packages.ListError && strings.HasPrefix(e.Msg, "# ") {
				continue // the compiler's report of the same errors, as raw output
			}
			file, line, ok := r.errorPosition(e.Pos)
			msg := e.Msg
			if !ok {
				if e.Pos != "" && e.Pos != "-" {
					msg = e.Pos + ": " + msg
				}
				msg = p.PkgPath + ": " + msg
			}
			r.diag(protocol.LevelError, msg, file, line)
		}
		if p.Module != nil && p.Module.Error != nil {
			r.diag(protocol.LevelError, "module "+p.Module.Path+": "+p.Module.Error.Err, "", 0)
		}
	})
}

// errorPosition parses "file:line:col" or "file:line" into a tree-relative
// file and a line.
func (r *run) errorPosition(pos string) (string, int, bool) {
	file, line := pos, 0
	for range 2 {
		i := strings.LastIndexByte(file, ':')
		if i < 0 {
			break
		}
		n, err := strconv.Atoi(file[i+1:])
		if err != nil {
			break
		}
		file, line = file[:i], n // ends as the first number: the line
	}
	rel, ok := r.tree.Rel(file)
	if !ok {
		return "", 0, false
	}
	return rel, max(line, 0), true
}

func (r *run) diag(level, msg, file string, line int) {
	r.result.Diagnostics = append(r.result.Diagnostics, protocol.Diagnostic{Level: level, Message: msg, File: file, Line: line})
}
