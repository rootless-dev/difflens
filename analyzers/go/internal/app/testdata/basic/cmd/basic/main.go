package main

import (
	"net/http"

	"example.com/basic/api"
)

func main() {
	_ = http.ListenAndServe(":8080", api.Routes(&api.Handler{}))
}
