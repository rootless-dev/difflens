// Package gin is a tiny stand-in for github.com/gin-gonic/gin, for tests.
package gin

type Context struct{}

type HandlerFunc func(*Context)

type RouterGroup struct{}

func (g *RouterGroup) GET(path string, handlers ...HandlerFunc) *RouterGroup   { return g }
func (g *RouterGroup) POST(path string, handlers ...HandlerFunc) *RouterGroup  { return g }
func (g *RouterGroup) Group(path string, handlers ...HandlerFunc) *RouterGroup { return g }
func (g *RouterGroup) Use(middleware ...HandlerFunc) *RouterGroup              { return g }

type Engine struct{ RouterGroup }

func New() *Engine { return &Engine{} }
