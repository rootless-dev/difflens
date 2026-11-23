package complexity

import (
	"go/ast"
	"go/parser"
	"go/token"
	"testing"
)

func TestCyclomatic(t *testing.T) {
	const src = `package p

func empty() {}

func ifElse(a, b bool) {
	if a && b {
	} else if a || b {
	}
}

func loops(xs []int) {
	for i := 0; i < 3; i++ {
	}
	for range xs {
	}
}

func switches(x int, c chan int) {
	switch x {
	case 1, 2:
	case 3:
	default:
	}
	select {
	case <-c:
	case c <- 1:
	default:
	}
}

func withClosure(a bool) {
	f := func() {
		if a {
		}
	}
	f()
}

func noBody()
`
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, "p.go", src, 0)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]int{
		"empty":       1,
		"ifElse":      5,
		"loops":       3,
		"switches":    5,
		"withClosure": 1,
		"noBody":      0,
	}
	for _, decl := range file.Decls {
		fd := decl.(*ast.FuncDecl)
		if got := Cyclomatic(fd.Body); got != want[fd.Name.Name] {
			t.Errorf("%s: got %d, want %d", fd.Name.Name, got, want[fd.Name.Name])
		}
	}
}
