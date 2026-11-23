package api

import (
	"net/http"

	"example.com/basic/service"
)

type Handler struct{ svc *service.Service }

func (h *Handler) Create(w http.ResponseWriter, r *http.Request) {
	_ = h.svc.Request(r.URL.Path)
}

type static struct{}

func (static) ServeHTTP(w http.ResponseWriter, r *http.Request) {}

func legacy(w http.ResponseWriter, r *http.Request) {}

const healthPattern = "GET /health"

// Routes registers handlers with Go 1.22 patterns.
func Routes(h *Handler) *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /refunds", h.Create)
	mux.HandleFunc(healthPattern, func(w http.ResponseWriter, r *http.Request) {})
	mux.Handle("/static/", static{})
	mux.Handle("/legacy", http.HandlerFunc(legacy))
	http.HandleFunc("/old", legacy)
	return mux
}
