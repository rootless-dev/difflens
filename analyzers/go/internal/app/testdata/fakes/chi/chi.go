// Package chi is a tiny stand-in for github.com/go-chi/chi/v5, for tests.
package chi

import "net/http"

type Router interface {
	http.Handler
	Use(middlewares ...func(http.Handler) http.Handler)
	With(middlewares ...func(http.Handler) http.Handler) Router
	Group(fn func(r Router)) Router
	Route(pattern string, fn func(r Router)) Router
	Mount(pattern string, h http.Handler)
	Get(pattern string, h http.HandlerFunc)
	Head(pattern string, h http.HandlerFunc)
	Post(pattern string, h http.HandlerFunc)
	Handle(pattern string, h http.Handler)
	Method(method, pattern string, h http.Handler)
	NotFound(h http.HandlerFunc)
}

type Mux struct{}

func NewRouter() *Mux { return &Mux{} }

func (m *Mux) ServeHTTP(w http.ResponseWriter, r *http.Request)           {}
func (m *Mux) Use(middlewares ...func(http.Handler) http.Handler)         {}
func (m *Mux) With(middlewares ...func(http.Handler) http.Handler) Router { return m }
func (m *Mux) Group(fn func(r Router)) Router                             { fn(m); return m }
func (m *Mux) Route(pattern string, fn func(r Router)) Router             { fn(m); return m }
func (m *Mux) Mount(pattern string, h http.Handler)                       {}
func (m *Mux) Get(pattern string, h http.HandlerFunc)                     {}
func (m *Mux) Head(pattern string, h http.HandlerFunc)                    {}
func (m *Mux) Post(pattern string, h http.HandlerFunc)                    {}
func (m *Mux) Handle(pattern string, h http.Handler)                      {}
func (m *Mux) Method(method, pattern string, h http.Handler)              {}
func (m *Mux) NotFound(h http.HandlerFunc)                                {}
