// Command difflens-analyzer-go is the DiffLens analyzer for Go source trees.
package main

import (
	"os"

	"github.com/rootless-dev/difflens/analyzers/go/internal/app"
)

func main() {
	os.Exit(app.Run(os.Args[1:], os.Stdout, os.Stderr))
}
