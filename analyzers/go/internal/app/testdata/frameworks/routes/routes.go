package routes

import (
	"net/http"

	"github.com/gin-gonic/gin"
	"github.com/go-chi/chi/v5"
)

type Refunds struct{}

func (Refunds) List(w http.ResponseWriter, r *http.Request)   {}
func (Refunds) Create(w http.ResponseWriter, r *http.Request) {}

func cancel(w http.ResponseWriter, r *http.Request) {}

func auth(c *gin.Context) {}

func show(c *gin.Context) {}

// Chi registers on the concrete mux and through the Router interface.
func Chi() {
	mux := chi.NewRouter()
	h := Refunds{}
	mux.Get("/refunds", h.List)
	var r chi.Router = mux
	r.Post("/refunds", h.Create)
	r.Method("DELETE", "/refunds/{id}", http.HandlerFunc(cancel))
	r.Get(routeFromSomewhere(), h.List) // not a literal: no entry point
}

func routeFromSomewhere() string { return "/dynamic" }

// Gin registers on the engine (promoted methods) and on a group.
func Gin() {
	e := gin.New()
	e.GET("/items/:id", auth, show)
	v1 := e.Group("/v1")
	v1.POST("/items", func(c *gin.Context) {})
}
