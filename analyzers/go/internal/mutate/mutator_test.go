package mutate

import (
	"go/ast"
	"go/importer"
	"go/parser"
	"go/token"
	"go/types"
	"slices"
	"strings"
	"testing"
)

// find returns the points of src, with type information when typed.
func find(t *testing.T, src string, typed bool) []Point {
	t.Helper()
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, "x.go", src, parser.ParseComments|parser.SkipObjectResolution)
	if err != nil {
		t.Fatal(err)
	}
	var info *types.Info
	if typed {
		info = &types.Info{Types: map[ast.Expr]types.TypeAndValue{}, Defs: map[*ast.Ident]types.Object{}, Uses: map[*ast.Ident]types.Object{}}
		conf := types.Config{Importer: importer.Default()}
		if _, err := conf.Check("example.com/x", fset, []*ast.File{f}, info); err != nil {
			t.Fatalf("type check: %v", err)
		}
	}
	return Find(fset, f, []byte(src), info)
}

// summary writes each point as "operator original -> mutated", with its
// Keep in brackets when it has one.
func summary(points []Point) []string {
	out := make([]string, 0, len(points))
	for _, p := range points {
		s := p.Operator + " " + p.Original + " -> " + p.Mutated
		if p.Keep != "" {
			s += " [" + strings.TrimSpace(p.Keep) + "]"
		}
		out = append(out, s)
	}
	return out
}

func requirePoints(t *testing.T, src string, typed bool, want []string) {
	t.Helper()
	got := summary(find(t, src, typed))
	if !slices.Equal(got, want) {
		t.Errorf("typed=%v:\n got %q\nwant %q", typed, got, want)
	}
}

func TestRelationalAndLogicalOperators(t *testing.T) {
	src := `package x

func rel(a, b int) bool {
	if a < b && b != 0 {
		return true
	}
	return false
}
`
	want := []string{
		"negation_removal a < b && b != 0 -> !(a < b && b != 0)",
		"relational_boundary < -> <=",
		"relational_negation < -> >=",
		"logical && -> ||",
		"relational_negation != -> ==",
		"return_value true -> false",
	}
	requirePoints(t, src, true, want)
	requirePoints(t, src, false, want)
}

func TestArithmeticAndIncrementOperators(t *testing.T) {
	src := `package x

func arith(a, b int, s string) (int, string) {
	a += b
	a -= b
	a++
	s += "x"
	return a*b + a%b - a/b, s + "y"
}
`
	// No arithmetic on strings: s += "x" and s + "y" stay.
	want := []string{
		"increment += -> -=",
		"increment -= -> +=",
		"increment ++ -> --",
		"return_value a*b + a%b - a/b -> 0 [_ = a*b + a%b - a/b;]",
		"arithmetic * -> /",
		"arithmetic + -> -",
		"arithmetic % -> *",
		"arithmetic - -> +",
		"arithmetic / -> *",
		`return_value s + "y" -> "" [_ = s + "y";]`,
	}
	requirePoints(t, src, true, want)
	requirePoints(t, src, false, want)
}

func TestStringSumsNeedTypesOrLiterals(t *testing.T) {
	src := `package x

func join(a, b string) string {
	a += b
	return a + b
}
`
	// Only types tell that a + b is a string.
	requirePoints(t, src, true, []string{`return_value a + b -> "" [_ = a + b;]`})
	requirePoints(t, src, false, []string{"increment += -> -=", `return_value a + b -> "" [_ = a + b;]`, "arithmetic + -> -"})
}

func TestNegations(t *testing.T) {
	src := `package x

func neg(ok bool, n int) bool {
	for ok && n > 0 {
		n--
	}
	if !ok {
		return false
	}
	if ok {
		return !ok
	}
	return ok
}
`
	// A condition that is already a negation or a comparison is not negated
	// as a whole: the other operators change it already.
	want := []string{
		"negation_removal ok && n > 0 -> !(ok && n > 0)",
		"logical && -> ||",
		"relational_boundary > -> >=",
		"relational_negation > -> <=",
		"increment -- -> ++",
		"negation_removal !ok -> ok",
		"negation_removal ok -> !ok",
		"return_value !ok -> false [_ = !ok;]",
		"negation_removal !ok -> ok",
		"return_value ok -> false [_ = ok;]",
	}
	requirePoints(t, src, true, want)
	requirePoints(t, src, false, want)
}

func TestReturnZeroValues(t *testing.T) {
	src := `package x

type point struct{ x int }
type celsius float64
type name string

var errBoom error

func z1() *point            { p := &point{}; return p }
func z2() point             { return point{x: 1} }
func z3() celsius           { return 1.5 }
func z4() name              { return "a" }
func z5() error             { return errBoom }
func z6() []int             { return []int{1} }
func z7[T any](v T) T       { return v }
func z8() (int, error)      { return z9() }
func z9() (int, error)      { return 1, nil }
func z10() [2]int           { return [2]int{1, 2} }
func z11() (n int)          { n = 1; return }
func z12() map[string]int   { return map[string]int{} }
func z13() int              { return 0 }
func z14() (a, b float64)   { return 1, 2 }
`
	requirePoints(t, src, true, []string{
		"return_value p -> nil [_ = p;]",
		"return_value point{x: 1} -> point{} [_ = point{x: 1};]",
		"return_value 1.5 -> 0",
		`return_value "a" -> ""`,
		"return_value errBoom -> nil [_ = errBoom;]",
		"return_value []int{1} -> nil [_ = []int{1};]",
		"return_value v -> *new(T) [_ = v;]",
		"return_value 1 -> 0",
		"return_value [2]int{1, 2} -> [2]int{} [_ = [2]int{1, 2};]",
		"return_value 1 -> 0",
		"return_value 2 -> 0",
	})
	// Without types, a named type could be anything.
	requirePoints(t, src, false, []string{
		"return_value p -> nil [_ = p;]",
		"return_value errBoom -> nil [_ = errBoom;]",
		"return_value []int{1} -> nil [_ = []int{1};]",
		"return_value 1 -> 0",
		"return_value [2]int{1, 2} -> [2]int{} [_ = [2]int{1, 2};]",
		"return_value 1 -> 0",
		"return_value 2 -> 0",
	})
}

func TestCallRemoval(t *testing.T) {
	src := `package x

import (
	"fmt"
	"log"
)

func calls() {
	fmt.Println("x")
	fmt.Fprintln(w, "x")
	log.Printf("x")
	s.logger.Info("x")
	s.Log().Warn("x")
	assert.Equal(t, 1, 2)
	panic("x")
	delete(m, k)
	process(&buf, nil, 1)
	s.mu.Lock()
	go run()
	defer done()
	notify()
}
`
	// Logging, assertions and panics stay; sync is only known with types.
	requirePoints(t, src, false, []string{
		`call_removal fmt.Fprintln(w, "x") ->  [_ = w;]`,
		"call_removal delete(m, k) ->  [_ = m; _ = k;]",
		"call_removal process(&buf, nil, 1) ->  [_ = buf;]",
		"call_removal s.mu.Lock() -> ",
		"call_removal notify() -> ",
	})
}

func TestCallRemovalWithTypes(t *testing.T) {
	src := `package x

import "sync"

type logger struct{}

func (logger) Info(string) {}

type S struct {
	mu  sync.Mutex
	wg  sync.WaitGroup
	out logger
	n   int
}

func (s *S) inc(m map[string]int, k string) {
	s.mu.Lock()
	s.n++
	s.mu.Unlock()
	s.wg.Done()
	s.out.Info("x")
	s.helper()
	delete(m, k)
}

func (s *S) helper() {}
`
	requirePoints(t, src, true, []string{
		"increment ++ -> --",
		"call_removal s.helper() ->  [_ = s;]",
		"call_removal delete(m, k) ->  [_ = m; _ = k;]",
	})
}

func TestDeclarationsLeftAlone(t *testing.T) {
	src := `package x

import "strings"

const c = 1 + 2

type arr [3 + 1]int

var v = 1 + 2

var f = func(a int) bool { return a > 1 }

func g() {
	const k = 2 * 3
	var a [2 * 2]int
	_ = a
	_ = strings.ToUpper
}
`
	points := find(t, src, true)
	if got := summary(points); !slices.Equal(got, []string{
		"arithmetic + -> -",
		"return_value a > 1 -> false [_ = a > 1;]",
		"relational_boundary > -> >=",
		"relational_negation > -> <=",
	}) {
		t.Fatalf("points: %q", got)
	}
	if len(points[0].Scopes) != 0 {
		t.Error("the var initializer is at top level")
	}
	for _, p := range points[1:] {
		if len(p.Scopes) != 1 {
			t.Errorf("%s: scopes %d, want the function literal", p.Description, len(p.Scopes))
		}
	}
}

func TestScopesAndDescriptions(t *testing.T) {
	src := `package x

func outer(xs []int) {
	apply(xs, func(x int) int {
		return x - 1
	})
}

func apply(xs []int, f func(int) int) {}
`
	points := find(t, src, true)
	if len(points) != 3 {
		t.Fatalf("points: %q", summary(points))
	}
	call, ret := points[0], points[1]
	if call.Operator != "call_removal" || len(call.Scopes) != 1 {
		t.Errorf("the call belongs to outer only: %+v", call)
	}
	if _, ok := ret.Scopes[0].(*ast.FuncLit); !ok || len(ret.Scopes) != 2 {
		t.Errorf("the return belongs to the closure, then outer: %d scopes", len(ret.Scopes))
	}
	if ret.Description != "return 0 instead of x - 1" || points[2].Description != "replace - with +" {
		t.Errorf("descriptions: %q, %q", ret.Description, points[2].Description)
	}
}

func TestApply(t *testing.T) {
	src := "package x\n\nfunc total(xs []int) int {\n\tt := 0\n\tfor _, x := range xs {\n\t\tt += x\n\t}\n\treturn t\n}\n"
	points := find(t, src, true)
	var ret Point
	for _, p := range points {
		if p.Operator == "return_value" {
			ret = p
		}
	}
	got := string(ret.Apply([]byte(src)))
	if want := strings.Replace(src, "\treturn t\n", "\t_ = t; return 0\n", 1); got != want {
		t.Errorf("applied:\n%s\nwant:\n%s", got, want)
	}
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, "x.go", got, 0)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := (&types.Config{}).Check("x", fset, []*ast.File{f}, nil); err != nil {
		t.Errorf("the mutant does not compile: %v", err)
	}
}

func TestShorten(t *testing.T) {
	if got := shorten("a +\n\t b"); got != "a + b" {
		t.Errorf("got %q", got)
	}
	long := strings.Repeat("é", 50)
	if got := shorten(long); got != strings.Repeat("é", 37)+"..." {
		t.Errorf("got %q", got)
	}
}
