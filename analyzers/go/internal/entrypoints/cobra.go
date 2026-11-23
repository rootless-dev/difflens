package entrypoints

import (
	"go/ast"
	"go/types"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

const cobraModule = "github.com/spf13/cobra"

// CobraDetector marks the Run and RunE functions of cobra.Command literals,
// labelled with the command name (the first word of Use), as registered by
// the function the literal is written in.
type CobraDetector struct{}

// Detect implements Detector.
func (CobraDetector) Detect(pkg *Package, funcs Resolver) []Found {
	var found []Found
	for _, file := range pkg.Files {
		var stack []ast.Node
		ast.Inspect(file.Syntax, func(n ast.Node) bool {
			if n == nil {
				stack = stack[:len(stack)-1]
				return true
			}
			stack = append(stack, n)
			lit, ok := n.(*ast.CompositeLit)
			if !ok || !isCobraCommand(pkg.Info.TypeOf(lit)) {
				return true
			}
			found = append(found, cobraCommand(pkg, file, lit, stack, funcs)...)
			return true
		})
	}
	return found
}

func isCobraCommand(t types.Type) bool {
	if t == nil {
		return false
	}
	if p, ok := t.(*types.Pointer); ok {
		t = p.Elem()
	}
	named, ok := t.(*types.Named)
	if !ok || named.Obj().Pkg() == nil {
		return false
	}
	return named.Obj().Name() == "Command" && isModuleRoot(named.Obj().Pkg().Path(), cobraModule)
}

func cobraCommand(pkg *Package, file File, lit *ast.CompositeLit, stack []ast.Node, funcs Resolver) []Found {
	label := ""
	var handlers []ast.Expr
	for _, elt := range lit.Elts {
		kv, ok := elt.(*ast.KeyValueExpr)
		if !ok {
			continue
		}
		key, ok := kv.Key.(*ast.Ident)
		if !ok {
			continue
		}
		switch key.Name {
		case "Use":
			if use, ok := constString(pkg.Info, kv.Value); ok {
				if words := strings.Fields(use); len(words) > 0 {
					label = words[0]
				}
			}
		case "Run", "RunE":
			handlers = append(handlers, kv.Value)
		}
	}
	if label == "" {
		return nil
	}
	regFile, regLine := position(pkg, file, lit.Pos())
	var found []Found
	for _, h := range handlers {
		for _, t := range handlerFuncs(pkg.Info, h, funcs, "") {
			found = append(found, Found{
				FuncID: t.id,
				EntryPoint: protocol.EntryPoint{
					Kind:  protocol.EntryCLICommand,
					Label: label,
					File:  regFile,
					Line:  regLine,
				},
				Registration: registrationAt(stack, funcs, regLine, "command "+label, t),
			})
		}
	}
	return found
}
