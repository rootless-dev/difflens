package mutate

import (
	"go/ast"
	"go/constant"
	"go/token"
	"go/types"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// Point is one mutation of a file: Original, the source text between the
// byte offsets Start and End, becomes Mutated.
type Point struct {
	Start, End int
	Original   string
	Mutated    string
	Operator   string
	// Description says what changes, without the function: "replace >= with >".
	Description string
	// Keep is written at the byte offset KeepAt (at most Start) along with
	// the mutation, "" for nothing: blank assignments ("_ = total; ") that
	// keep using the variables the mutation stops using, so that the mutant
	// still compiles ("declared and not used"). They evaluate what the
	// original evaluated, so the mutant changes only what it says; the
	// report leaves them out.
	Keep   string
	KeepAt int
	// Scopes are the functions holding the point (*ast.FuncDecl and
	// *ast.FuncLit), innermost first; Decl is the top-level declaration
	// holding it.
	Scopes []ast.Node
	Decl   ast.Decl
}

// Find returns the mutation points of file, parsed with fset from src, in
// source order. info is the file's type information; it may be nil or
// partial, and then the operators that depend on types fall back on syntax:
// `+` and `+=` are left alone when an operand looks like a string, and a
// result whose zero value cannot be told from its type expression is not
// mutated.
//
// The operators (mutation.schema.json, rule 2):
//
//	relational_boundary  <  <=  >  >=  into their neighbour (< and <=, > and >=)
//	relational_negation  == and !=, < and >=, > and <=, into each other
//	arithmetic           + and -, * and /, % into *   (on numbers only)
//	logical              && and || into each other
//	negation_removal     !x into x; a whole if or for condition c into !(c),
//	                     unless it is already a comparison or a negation,
//	                     which the operators above change already
//	return_value         a returned expression into the zero value of its
//	                     result type (0, "", false, nil, T{}, *new(T) for a
//	                     type parameter), unless it is that already
//	call_removal         a statement that is only a call, removed
//	increment            ++ and --, += and -= into each other
//
// Not mutated: constant, type and import declarations, type expressions
// (array lengths, generic constraints), and the calls keepCall keeps
// (logging, assertions, panics, sync).
func Find(fset *token.FileSet, file *ast.File, src []byte, info *types.Info) []Point {
	f := &finder{tf: fset.File(file.Pos()), src: src, info: info}
	if f.tf == nil {
		return nil
	}
	for _, decl := range file.Decls {
		if gd, ok := decl.(*ast.GenDecl); ok && gd.Tok != token.VAR {
			continue
		}
		f.decl = decl
		f.stack = f.stack[:0]
		ast.Inspect(decl, f.visit)
	}
	slices.SortStableFunc(f.points, func(a, b Point) int { return a.Start - b.Start })
	return f.points
}

type finder struct {
	tf     *token.File
	src    []byte
	info   *types.Info
	decl   ast.Decl
	stack  []ast.Node // ancestors of the node being visited
	points []Point
}

func (f *finder) visit(n ast.Node) bool {
	if n == nil {
		f.stack = f.stack[:len(f.stack)-1]
		return true
	}
	switch n := n.(type) {
	case *ast.GenDecl:
		if n.Tok != token.VAR {
			return false
		}
	case *ast.ArrayType, *ast.StructType, *ast.InterfaceType, *ast.FuncType, *ast.MapType, *ast.ChanType:
		return false
	case *ast.BinaryExpr:
		f.binary(n)
	case *ast.UnaryExpr:
		if n.Op == token.NOT {
			f.add(n.Pos(), n.End(), f.text(n.X), protocol.OperatorNegationRemoval,
				"replace "+shorten(f.text(n))+" with "+shorten(f.text(n.X)))
		}
	case *ast.IfStmt:
		f.condition(n.Cond, "if")
	case *ast.ForStmt:
		f.condition(n.Cond, "for")
	case *ast.ReturnStmt:
		f.returns(n)
	case *ast.ExprStmt:
		if call, ok := ast.Unparen(n.X).(*ast.CallExpr); ok && !f.keepCall(call) {
			p := f.add(n.Pos(), n.End(), "", protocol.OperatorCallRemoval, "remove the call to "+shorten(f.text(call.Fun)))
			p.Keep, p.KeepAt = f.keepVars(call), p.Start
		}
	case *ast.IncDecStmt:
		to := token.DEC
		if n.Tok == token.DEC {
			to = token.INC
		}
		f.swap(n.TokPos, n.Tok, to, protocol.OperatorIncrement)
	case *ast.AssignStmt:
		f.assign(n)
	}
	f.stack = append(f.stack, n)
	return true
}

// Operator replacements of binary expressions.
var (
	boundary = map[token.Token]token.Token{token.LSS: token.LEQ, token.LEQ: token.LSS, token.GTR: token.GEQ, token.GEQ: token.GTR}
	negation = map[token.Token]token.Token{
		token.EQL: token.NEQ, token.NEQ: token.EQL,
		token.LSS: token.GEQ, token.GEQ: token.LSS, token.GTR: token.LEQ, token.LEQ: token.GTR,
	}
	logical    = map[token.Token]token.Token{token.LAND: token.LOR, token.LOR: token.LAND}
	arithmetic = map[token.Token]token.Token{token.ADD: token.SUB, token.SUB: token.ADD, token.MUL: token.QUO, token.QUO: token.MUL, token.REM: token.MUL}
)

func (f *finder) binary(n *ast.BinaryExpr) {
	if to, ok := boundary[n.Op]; ok {
		f.swap(n.OpPos, n.Op, to, protocol.OperatorRelationalBoundary)
	}
	if to, ok := negation[n.Op]; ok {
		f.swap(n.OpPos, n.Op, to, protocol.OperatorRelationalNegation)
	}
	if to, ok := logical[n.Op]; ok {
		f.swap(n.OpPos, n.Op, to, protocol.OperatorLogical)
	}
	if to, ok := arithmetic[n.Op]; ok && (n.Op != token.ADD || f.numeric(n, n.X, n.Y)) {
		f.swap(n.OpPos, n.Op, to, protocol.OperatorArithmetic)
	}
}

// assign swaps += and -=, += only on numbers.
func (f *finder) assign(n *ast.AssignStmt) {
	switch {
	case len(n.Lhs) != 1 || len(n.Rhs) != 1:
	case n.Tok == token.SUB_ASSIGN:
		f.swap(n.TokPos, n.Tok, token.ADD_ASSIGN, protocol.OperatorIncrement)
	case n.Tok == token.ADD_ASSIGN && f.numeric(n.Lhs[0], n.Rhs[0]):
		f.swap(n.TokPos, n.Tok, token.SUB_ASSIGN, protocol.OperatorIncrement)
	}
}

// numeric reports whether expr has a numeric type. Without type information
// it reports whether none of the operands looks like a string.
func (f *finder) numeric(expr ast.Expr, operands ...ast.Expr) bool {
	if t := f.typeOf(expr); t != nil {
		b, ok := t.Underlying().(*types.Basic)
		return ok && b.Info()&types.IsNumeric != 0
	}
	for _, op := range operands {
		if mayBeString(op) {
			return false
		}
	}
	return true
}

// mayBeString reports whether expr is a string literal or a sum holding one.
func mayBeString(expr ast.Expr) bool {
	switch e := ast.Unparen(expr).(type) {
	case *ast.BasicLit:
		return e.Kind == token.STRING
	case *ast.BinaryExpr:
		return e.Op == token.ADD && (mayBeString(e.X) || mayBeString(e.Y))
	}
	return false
}

// condition negates a whole if or for condition.
func (f *finder) condition(cond ast.Expr, keyword string) {
	if cond == nil {
		return
	}
	switch c := ast.Unparen(cond).(type) {
	case *ast.UnaryExpr:
		if c.Op == token.NOT {
			return
		}
	case *ast.BinaryExpr:
		if _, ok := negation[c.Op]; ok {
			return
		}
	}
	text := f.text(cond)
	mutated := "!(" + text + ")"
	switch cond.(type) {
	case *ast.Ident, *ast.CallExpr, *ast.SelectorExpr, *ast.IndexExpr, *ast.ParenExpr:
		mutated = "!" + text
	}
	f.add(cond.Pos(), cond.End(), mutated, protocol.OperatorNegationRemoval, "negate the "+keyword+" condition "+shorten(text))
}

// returns replaces each returned expression with the zero value of its
// result type.
func (f *finder) returns(n *ast.ReturnStmt) {
	ft := f.funcType()
	if ft == nil || ft.Results == nil || len(n.Results) == 0 {
		return
	}
	var results []ast.Expr
	for _, field := range ft.Results.List {
		for range max(1, len(field.Names)) {
			results = append(results, field.Type)
		}
	}
	if len(results) != len(n.Results) {
		return // return f() of a function with several results
	}
	for i, expr := range n.Results {
		zero, ok := f.zero(results[i])
		if !ok || f.isZero(expr, zero) {
			continue
		}
		p := f.add(expr.Pos(), expr.End(), zero, protocol.OperatorReturnValue,
			"return "+zero+" instead of "+shorten(f.text(expr)))
		if !f.constant(expr) {
			p.Keep, p.KeepAt = "_ = "+f.text(expr)+"; ", f.tf.Offset(n.Pos())
		}
	}
}

// constant reports whether expr is a literal, true or false or, with
// types, a constant.
func (f *finder) constant(expr ast.Expr) bool {
	switch e := ast.Unparen(expr).(type) {
	case *ast.BasicLit:
		return true
	case *ast.Ident:
		if f.info == nil && (e.Name == "true" || e.Name == "false") {
			return true
		}
	}
	return f.info != nil && f.info.Types[expr].Value != nil
}

// keepVars returns the blank assignments that keep using the variables a
// removed call used: its arguments and the value its method is called on,
// when they are variables (`process(&buf)` keeps buf). Without types only
// arguments are kept, since the value a method is called on may be a
// package name.
func (f *finder) keepVars(call *ast.CallExpr) string {
	var keep strings.Builder
	seen := map[string]bool{}
	use := func(expr ast.Expr, needTypes bool) {
		id := rootIdent(expr)
		if id == nil || seen[id.Name] || !f.isVar(id, needTypes) {
			return
		}
		seen[id.Name] = true
		keep.WriteString("_ = " + id.Name + "; ")
	}
	if sel, ok := ast.Unparen(call.Fun).(*ast.SelectorExpr); ok {
		use(sel.X, true)
	}
	for _, arg := range call.Args {
		use(arg, false)
	}
	return keep.String()
}

// rootIdent returns the variable expr is made from: x for x, &x, *x, x.f,
// x[i] and (x), or nil.
func rootIdent(expr ast.Expr) *ast.Ident {
	for {
		switch e := expr.(type) {
		case *ast.Ident:
			return e
		case *ast.ParenExpr:
			expr = e.X
		case *ast.UnaryExpr:
			if e.Op != token.AND {
				return nil
			}
			expr = e.X
		case *ast.StarExpr:
			expr = e.X
		case *ast.SelectorExpr:
			expr = e.X
		case *ast.IndexExpr:
			expr = e.X
		default:
			return nil
		}
	}
}

// isVar reports whether id names a variable. Without types, it reports
// whether it is not a predeclared constant, unless needTypes.
func (f *finder) isVar(id *ast.Ident, needTypes bool) bool {
	if id.Name == "_" {
		return false
	}
	if f.info != nil {
		if obj := f.info.Uses[id]; obj != nil {
			_, ok := obj.(*types.Var)
			return ok
		}
		return false
	}
	switch id.Name {
	case "nil", "true", "false", "iota":
		return false
	}
	return !needTypes
}

// funcType returns the type of the innermost function being visited.
func (f *finder) funcType() *ast.FuncType {
	for i := len(f.stack) - 1; i >= 0; i-- {
		switch fn := f.stack[i].(type) {
		case *ast.FuncDecl:
			return fn.Type
		case *ast.FuncLit:
			return fn.Type
		}
	}
	return nil
}

// zero returns the zero value of the type expression typ, as Go source.
func (f *finder) zero(typ ast.Expr) (string, bool) {
	if t := f.typeOf(typ); t != nil {
		return zeroOf(t, f.text(typ))
	}
	return syntacticZero(typ, f.text(typ))
}

func zeroOf(t types.Type, text string) (string, bool) {
	t = types.Unalias(t)
	if _, ok := t.(*types.TypeParam); ok {
		return "*new(" + text + ")", true
	}
	switch u := t.Underlying().(type) {
	case *types.Basic:
		switch {
		case u.Info()&types.IsBoolean != 0:
			return "false", true
		case u.Info()&types.IsString != 0:
			return `""`, true
		case u.Info()&types.IsNumeric != 0:
			return "0", true
		case u.Kind() == types.UnsafePointer:
			return "nil", true
		}
	case *types.Pointer, *types.Slice, *types.Map, *types.Chan, *types.Signature, *types.Interface:
		return "nil", true
	case *types.Struct, *types.Array:
		return text + "{}", true
	}
	return "", false
}

// syntacticZero is zeroOf for a type expression without type information:
// predeclared types and type literals only, since a named type could be
// anything.
func syntacticZero(typ ast.Expr, text string) (string, bool) {
	switch t := typ.(type) {
	case *ast.Ident:
		switch t.Name {
		case "bool":
			return "false", true
		case "string":
			return `""`, true
		case "error", "any":
			return "nil", true
		case "int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr",
			"byte", "rune", "float32", "float64", "complex64", "complex128":
			return "0", true
		}
	case *ast.ParenExpr:
		return syntacticZero(t.X, text)
	case *ast.StarExpr, *ast.MapType, *ast.ChanType, *ast.FuncType, *ast.InterfaceType:
		return "nil", true
	case *ast.ArrayType:
		if t.Len == nil {
			return "nil", true
		}
		return text + "{}", true
	case *ast.StructType:
		return text + "{}", true
	}
	return "", false
}

// isZero reports whether expr is already the zero value zero: written the
// same, nil, an empty composite literal, or a constant equal to zero.
func (f *finder) isZero(expr ast.Expr, zero string) bool {
	e := ast.Unparen(expr)
	if f.text(e) == zero {
		return true
	}
	switch e := e.(type) {
	case *ast.Ident:
		if e.Name == "nil" {
			return true
		}
	case *ast.CompositeLit:
		if len(e.Elts) == 0 {
			return true
		}
	}
	if f.info == nil {
		return false
	}
	v := f.info.Types[expr].Value
	switch {
	case v == nil:
		return false
	case v.Kind() == constant.Bool:
		return !constant.BoolVal(v)
	case v.Kind() == constant.String:
		return constant.StringVal(v) == ""
	case v.Kind() == constant.Int, v.Kind() == constant.Float:
		return constant.Sign(v) == 0
	}
	return false
}

// typeOf returns the type of expr, or nil when it is not known.
func (f *finder) typeOf(expr ast.Expr) types.Type {
	if f.info == nil {
		return nil
	}
	t := f.info.TypeOf(expr)
	if t == nil || t == types.Typ[types.Invalid] {
		return nil
	}
	return t
}

// swap replaces the operator token at pos with to.
func (f *finder) swap(pos token.Pos, from, to token.Token, operator string) {
	end := pos + token.Pos(len(from.String()))
	f.add(pos, end, to.String(), operator, "replace "+from.String()+" with "+to.String())
}

// add records a point and returns it, for the caller to complete.
func (f *finder) add(pos, end token.Pos, mutated, operator, description string) *Point {
	start, stop := f.tf.Offset(pos), f.tf.Offset(end)
	p := Point{
		Start: start, End: stop, Original: string(f.src[start:stop]), Mutated: mutated,
		Operator: operator, Description: description, Decl: f.decl,
	}
	for i := len(f.stack) - 1; i >= 0; i-- {
		switch f.stack[i].(type) {
		case *ast.FuncDecl, *ast.FuncLit:
			p.Scopes = append(p.Scopes, f.stack[i])
		}
	}
	f.points = append(f.points, p)
	return &f.points[len(f.points)-1]
}

// Apply returns src with the point applied, its Keep included.
func (p Point) Apply(src []byte) []byte {
	if p.Keep == "" {
		return slices.Concat(src[:p.Start], []byte(p.Mutated), src[p.End:])
	}
	return slices.Concat(src[:p.KeepAt], []byte(p.Keep), src[p.KeepAt:p.Start], []byte(p.Mutated), src[p.End:])
}

// text returns the source text of node.
func (f *finder) text(node ast.Node) string {
	return string(f.src[f.tf.Offset(node.Pos()):f.tf.Offset(node.End())])
}

// shorten makes source text fit a description: one line, at most 40
// characters.
func shorten(s string) string {
	s = strings.Join(strings.Fields(s), " ")
	if utf8.RuneCountInString(s) <= 40 {
		return s
	}
	return string([]rune(s)[:37]) + "..."
}
