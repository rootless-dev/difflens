package mutate

import (
	"go/ast"
	"go/types"
	"path"
	"strings"
)

// quietNames mark a call as logging or as an assertion when they name the
// value the method is called on, or anything it is reached through
// (log.Printf, s.logger.Info, r.Log().Warn, assert.Equal), or, with types,
// the last element of the called function's package path. A removed log
// call survives for no good reason, and an assertion belongs to a test.
var quietNames = map[string]bool{
	"log": true, "logger": true, "logging": true, "slog": true, "logrus": true, "zap": true, "zerolog": true,
	"klog": true, "glog": true, "assert": true, "require": true, "testing": true,
}

// keepBuiltins are the builtins whose calls are not removed: panic only
// leaves a function without its terminating statement (a mutant that does
// not compile), print and println are logging, and recover is a statement
// only in deferred functions, where removing it turns a handled panic into
// a crash that tells nothing about the tests. close, delete, copy and clear
// are removed like any call.
var keepBuiltins = map[string]bool{"panic": true, "print": true, "println": true, "recover": true}

// keepCall reports whether a call statement must not be removed: logging
// and assertions (quietNames, and fmt.Print, fmt.Printf and fmt.Println,
// which write to stdout; fmt.Fprint* write to a writer the code chose and
// are removed), the builtins of keepBuiltins, methods whose receiver type is
// named like a logger, and, with types, the functions of package sync:
// without Unlock or Done the mutant deadlocks, which costs a timeout and
// says nothing about the tests.
func (f *finder) keepCall(call *ast.CallExpr) bool {
	fun := ast.Unparen(call.Fun)
	if ix, ok := fun.(*ast.IndexExpr); ok { // an explicit instantiation, f[T](x)
		fun = ast.Unparen(ix.X)
	}
	if ix, ok := fun.(*ast.IndexListExpr); ok {
		fun = ast.Unparen(ix.X)
	}
	switch fn := fun.(type) {
	case *ast.Ident:
		if !keepBuiltins[fn.Name] {
			return false
		}
		if f.info == nil {
			return true
		}
		_, builtin := f.info.Uses[fn].(*types.Builtin)
		return builtin
	case *ast.SelectorExpr:
		if pkg, ok := ast.Unparen(fn.X).(*ast.Ident); ok && pkg.Name == "fmt" &&
			(fn.Sel.Name == "Print" || fn.Sel.Name == "Printf" || fn.Sel.Name == "Println") {
			return true
		}
		if quietChain(fn.X) {
			return true
		}
		return f.quietFunc(fn.Sel)
	}
	return false
}

// quietChain reports whether a quiet name appears in expr, the value a
// method is called on, or anything it is reached through.
func quietChain(expr ast.Expr) bool {
	for {
		switch e := expr.(type) {
		case *ast.Ident:
			return quietNames[strings.ToLower(e.Name)]
		case *ast.SelectorExpr:
			if quietNames[strings.ToLower(e.Sel.Name)] {
				return true
			}
			expr = e.X
		case *ast.CallExpr:
			expr = e.Fun
		case *ast.IndexExpr:
			expr = e.X
		case *ast.ParenExpr:
			expr = e.X
		case *ast.StarExpr:
			expr = e.X
		default:
			return false
		}
	}
}

// quietFunc applies the rules that need types to the function sel names.
func (f *finder) quietFunc(sel *ast.Ident) bool {
	if f.info == nil {
		return false
	}
	fn, ok := f.info.Uses[sel].(*types.Func)
	if !ok || fn.Pkg() == nil {
		return false
	}
	pkgPath := fn.Pkg().Path()
	if pkgPath == "sync" || quietNames[path.Base(pkgPath)] {
		return true
	}
	sig, ok := fn.Type().(*types.Signature)
	if !ok || sig.Recv() == nil {
		return false
	}
	recv := sig.Recv().Type()
	if p, ok := recv.(*types.Pointer); ok {
		recv = p.Elem()
	}
	if named, ok := types.Unalias(recv).(*types.Named); ok {
		return strings.Contains(strings.ToLower(named.Obj().Name()), "logger")
	}
	return false
}
