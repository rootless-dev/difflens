package routes

import (
	"net/http"

	"github.com/gin-gonic/gin"
	"github.com/go-chi/chi/v5"
)

const (
	apiPrefix   = "/api"
	itemsPath   = "/items"
	itemParam   = "id"
	itemPattern = itemsPath + "/{" + itemParam + "}"
)

type Items struct{}

func (Items) List(w http.ResponseWriter, r *http.Request)   {}
func (Items) Show(w http.ResponseWriter, r *http.Request)   {}
func (Items) Create(w http.ResponseWriter, r *http.Request) {}
func (Items) Admin(w http.ResponseWriter, r *http.Request)  {}
func (Items) Vars(w http.ResponseWriter, r *http.Request)   {}
func (Items) ByID(w http.ResponseWriter, r *http.Request)   {}

// Getter is a router interface declared in the tree, as a package that does
// not import chi declares it; *chi.Mux satisfies it.
type Getter interface {
	Get(pattern string, h http.HandlerFunc)
}

// Mount registers through Getter, with a path folded from constants.
func (h Items) Mount(r Getter) {
	r.Get(apiPrefix+itemPattern, h.Show)
}

func logRequests(next http.Handler) http.Handler { return next }

type Auth struct{}

func (Auth) Check(next http.Handler) http.Handler { return next }

// RequestID is a middleware constructor: the closure it returns is the
// middleware.
func RequestID(header string) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler { return next }
}

var shared func(http.Handler) http.Handler

// Shared returns a middleware the analyzer cannot resolve.
func Shared() func(http.Handler) http.Handler { return shared }

func notFound(w http.ResponseWriter, r *http.Request) {}

// ChiFlow nests routes under constant prefixes.
func ChiFlow(prefix string, h Items, a Auth) http.Handler {
	r := chi.NewRouter()
	r.Use(logRequests, a.Check, RequestID("X-Request-Id"), Shared())
	r.Route(apiPrefix, func(api chi.Router) {
		api.Get(itemsPath, h.List)
		api.With(a.Check).Post(itemsPath, h.Create)
		api.Group(func(admin chi.Router) {
			admin.Use(logRequests)
			admin.Get("/admin", h.Admin)
		})
	})
	debug := chi.NewRouter()
	debug.Get("/vars", h.Vars)
	r.Mount("/debug", debug)
	r.Get(prefix+"/{id}", h.ByID)
	r.NotFound(notFound)
	return r
}

func withLogging(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) { next(w, r) }
}

func serveItem(w http.ResponseWriter, r *http.Request) {}

// NetHTTP registers a converted function and a wrapped handler.
func NetHTTP(h Items) {
	mux := http.NewServeMux()
	mux.Handle("GET /item", http.HandlerFunc(serveItem))
	mux.HandleFunc("POST /items", withLogging(h.Create))
}

func ginLogger(c *gin.Context)     {}
func ginAuth(c *gin.Context)       {}
func ginList(c *gin.Context)       {}
func ginCreateUser(c *gin.Context) {}

// GinFlow registers middlewares and nested groups.
func GinFlow() {
	e := gin.New()
	e.Use(ginLogger)
	v2 := e.Group("/v2", ginAuth)
	v2.GET(itemsPath, ginList)
	v2.Group("/admin").POST("/users", ginCreateUser)
}

func run(f func()) { f() }

func task() {}

type Server struct{ handle http.HandlerFunc }

// References uses functions as values without a known meaning.
func References(h Items) Server {
	run(task)
	h.List(nil, nil)
	return Server{handle: h.Show}
}
