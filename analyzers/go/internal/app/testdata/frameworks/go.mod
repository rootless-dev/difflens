module example.com/frameworks

go 1.24

require (
	github.com/gin-gonic/gin v1.0.0
	github.com/go-chi/chi/v5 v5.0.0
	github.com/spf13/cobra v1.0.0
)

replace (
	github.com/gin-gonic/gin => ../fakes/gin
	github.com/go-chi/chi/v5 => ../fakes/chi
	github.com/spf13/cobra => ../fakes/cobra
)
