package analyzer

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
)

// OpenTree checks that dir is a directory and returns the tree rooted at it
// with the units the go command has to load or run in it.
func OpenTree(dir string) (Tree, []loader.Unit, error) {
	info, err := os.Stat(dir)
	if err != nil {
		return Tree{}, nil, err
	}
	if !info.IsDir() {
		return Tree{}, nil, fmt.Errorf("%s is not a directory", dir)
	}
	tree, err := NewTree(dir)
	if err != nil {
		return Tree{}, nil, err
	}
	units, err := loader.Discover(tree.Root)
	if err != nil {
		return Tree{}, nil, err
	}
	return tree, units, nil
}

// Tree is the analysed source tree. It turns absolute file names reported by
// the go command into paths relative to the tree, with "/" separators.
type Tree struct {
	Root  string
	roots []string // Root, plus its symlink-free form when different
}

// NewTree returns the tree rooted at dir.
func NewTree(dir string) (Tree, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return Tree{}, fmt.Errorf("resolve %s: %w", dir, err)
	}
	t := Tree{Root: abs, roots: []string{abs}}
	if real, err := filepath.EvalSymlinks(abs); err == nil && real != abs {
		t.roots = append(t.roots, real)
	}
	return t, nil
}

// Rel returns filename relative to the tree, and false when it lies outside.
func (t Tree) Rel(filename string) (string, bool) {
	if filename == "" || !filepath.IsAbs(filename) {
		return "", false
	}
	for _, root := range t.roots {
		rel, err := filepath.Rel(root, filename)
		if err == nil && filepath.IsLocal(rel) {
			return filepath.ToSlash(rel), true
		}
	}
	return "", false
}
