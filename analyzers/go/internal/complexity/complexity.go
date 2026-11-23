// Package complexity computes cyclomatic complexity from Go syntax.
package complexity

import (
	"go/ast"
	"go/token"
)

// Cyclomatic returns 1 plus the number of decision points in body: if, for,
// range, case clauses other than default, select cases other than default,
// && and ||. Function literals inside body are separate functions and are not
// counted. It returns 0 for a nil body (a function declared without one).
func Cyclomatic(body *ast.BlockStmt) int {
	if body == nil {
		return 0
	}
	n := 1
	ast.Inspect(body, func(node ast.Node) bool {
		switch node := node.(type) {
		case *ast.FuncLit:
			return false
		case *ast.IfStmt, *ast.ForStmt, *ast.RangeStmt:
			n++
		case *ast.CaseClause:
			if node.List != nil {
				n++
			}
		case *ast.CommClause:
			if node.Comm != nil {
				n++
			}
		case *ast.BinaryExpr:
			if node.Op == token.LAND || node.Op == token.LOR {
				n++
			}
		}
		return true
	})
	return n
}
