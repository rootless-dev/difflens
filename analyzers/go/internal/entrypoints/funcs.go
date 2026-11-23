package entrypoints

import (
	"go/ast"
	"path"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// MainDetector marks func main of package main, labelled with the command
// name (the last element of the package path).
type MainDetector struct{}

// Detect implements Detector.
func (MainDetector) Detect(pkg *Package, funcs Resolver) []Found {
	if pkg.Name != "main" {
		return nil
	}
	var found []Found
	for _, file := range pkg.Files {
		if file.IsTest {
			continue
		}
		for _, decl := range file.Syntax.Decls {
			fd, ok := decl.(*ast.FuncDecl)
			if !ok || fd.Recv != nil || fd.Name.Name != "main" {
				continue
			}
			if id, ok := funcs.FuncIDAt(fd.Name.Pos()); ok {
				found = append(found, Found{FuncID: id, EntryPoint: protocol.EntryPoint{
					Kind:  protocol.EntryMain,
					Label: path.Base(pkg.Path),
				}})
			}
		}
	}
	return found
}

// TestDetector marks the functions go test runs: Test*, Benchmark*, Fuzz*
// and Example* in _test.go files, plus TestMain.
type TestDetector struct{}

var testPrefixes = []string{"Test", "Benchmark", "Fuzz", "Example"}

// Detect implements Detector.
func (TestDetector) Detect(pkg *Package, funcs Resolver) []Found {
	var found []Found
	for _, file := range pkg.Files {
		if !file.IsTest {
			continue
		}
		for _, decl := range file.Syntax.Decls {
			fd, ok := decl.(*ast.FuncDecl)
			if !ok || fd.Recv != nil || !isTestName(fd.Name.Name) {
				continue
			}
			if id, ok := funcs.FuncIDAt(fd.Name.Pos()); ok {
				found = append(found, Found{FuncID: id, EntryPoint: protocol.EntryPoint{
					Kind:  protocol.EntryTest,
					Label: fd.Name.Name,
				}})
			}
		}
	}
	return found
}

// isTestName applies go test's rule: a known prefix followed by nothing or by
// a character that is not a lower-case letter.
func isTestName(name string) bool {
	for _, prefix := range testPrefixes {
		rest, ok := strings.CutPrefix(name, prefix)
		if !ok {
			continue
		}
		if rest == "" {
			return true
		}
		r, _ := utf8.DecodeRuneInString(rest)
		return !unicode.IsLower(r)
	}
	return false
}
