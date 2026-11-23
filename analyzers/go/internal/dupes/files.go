package dupes

import (
	"bytes"
	"cmp"
	"context"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
)

// maxFileSize is the size above which a file is left out.
const maxFileSize = 1 << 20

// file is one source file of the search.
type file struct {
	path   string // relative to the tree, "/" separators
	tokens []tok
	added  lineSet
	// touch[i] is the number of tokens before token i that start on an
	// added line, so that whether a window touches the change is O(1).
	touch []int32
}

// touches reports whether the n tokens from token start include one that
// starts on an added line.
func (f *file) touches(start, n int32) bool {
	return f.touch[start+n] > f.touch[start]
}

// lines returns the lines spanned by the n tokens from token start.
func (f *file) lines(start, n int32) (int, int) {
	return int(f.tokens[start].Start), int(f.tokens[start+n-1].End)
}

// skipDir reports whether the walk leaves out the folder name, as
// loader.Discover does and as the go command ignores them.
func skipDir(name string) bool {
	return name == "vendor" || name == "testdata" || name == "node_modules" ||
		strings.HasPrefix(name, ".") || strings.HasPrefix(name, "_")
}

// excludesDir reports whether a folder named name, below the root of the
// tree, leaves out every file under it.
func (e Exclude) excludesDir(name string) bool {
	return slices.ContainsFunc(e.Dirs, func(d string) bool { return d != "" && equalFoldASCII(d, name) })
}

// excludesFile reports whether a file named name is left out by its
// extension.
func (e Exclude) excludesFile(name string) bool {
	return slices.ContainsFunc(e.Extensions, func(ext string) bool {
		return ext != "" && len(name) >= len(ext) && equalFoldASCII(name[len(name)-len(ext):], ext)
	})
}

// equalFoldASCII reports whether a and b are equal ignoring the case of
// ASCII letters only, as the schema compares names.
func equalFoldASCII(a, b string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range len(a) {
		if lowerASCII(a[i]) != lowerASCII(b[i]) {
			return false
		}
	}
	return true
}

func lowerASCII(c byte) byte {
	if 'A' <= c && c <= 'Z' {
		return c + 'a' - 'A'
	}
	return c
}

// loadFiles reads and tokenizes the Go files of the tree at root, sorted by
// path, without those exclude leaves out. A file that cannot be read or
// tokenized is a diagnostic in res.
func loadFiles(ctx context.Context, log logging.Logger, root string, added Added, exclude Exclude, res *Result) ([]*file, error) {
	var paths []string
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			if path == root {
				return err
			}
			res.Diagnostics = append(res.Diagnostics, fmt.Sprintf("could not read %s: %v", relPath(root, path), err))
			return nil
		}
		if d.IsDir() {
			// The root is not a folder of the relative paths, so neither
			// rule applies to its name.
			if path != root && (skipDir(d.Name()) || exclude.excludesDir(d.Name())) {
				return filepath.SkipDir
			}
			return nil
		}
		if d.Type().IsRegular() && strings.HasSuffix(d.Name(), ".go") && !exclude.excludesFile(d.Name()) {
			paths = append(paths, path)
		}
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("scan %s: %w", root, err)
	}

	var files []*file
	for _, path := range paths {
		if ctx.Err() != nil {
			break
		}
		rel := relPath(root, path)
		f, err := loadFile(path, rel, added[rel])
		switch {
		case err != nil:
			res.Diagnostics = append(res.Diagnostics, err.Error())
		case f == nil:
			log.Debug("file left out", "file", rel)
		default:
			files = append(files, f)
		}
	}
	// The walk is lexical per folder, which is not the order of the paths.
	slices.SortFunc(files, func(a, b *file) int { return cmp.Compare(a.path, b.path) })
	return files, nil
}

// loadFile reads and tokenizes one file. It returns nil without an error for
// a file that is left out: too large or generated.
func loadFile(path, rel string, added [][2]int) (*file, error) {
	info, err := os.Stat(path)
	if err != nil {
		return nil, fmt.Errorf("could not read %s: %v", rel, err)
	}
	if info.Size() > maxFileSize {
		return nil, nil
	}
	src, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("could not read %s: %v", rel, err)
	}
	if isGenerated(src) {
		return nil, nil
	}
	toks, err := tokenize(src)
	if err != nil {
		return nil, fmt.Errorf("could not tokenize %s: %v", rel, err)
	}
	f := &file{path: rel, tokens: toks, added: newLineSet(bytes.Count(src, []byte("\n"))+1, added)}
	f.touch = make([]int32, len(toks)+1)
	for i, t := range toks {
		f.touch[i+1] = f.touch[i]
		if f.added.has(int(t.Start)) {
			f.touch[i+1]++
		}
	}
	return f, nil
}

// isGenerated applies the convention analyze uses: a "Code generated ...
// DO NOT EDIT." comment before the package clause.
func isGenerated(src []byte) bool {
	f, err := parser.ParseFile(token.NewFileSet(), "", src, parser.PackageClauseOnly|parser.ParseComments)
	return err == nil && ast.IsGenerated(f)
}

func relPath(root, path string) string {
	rel, err := filepath.Rel(root, path)
	if err != nil {
		return filepath.ToSlash(path)
	}
	return filepath.ToSlash(rel)
}

// lineSet is the added lines of a file.
type lineSet struct {
	prefix []int32 // prefix[l] is the number of added lines in 1..l
}

// newLineSet returns the set of the inclusive ranges, clipped to the n lines
// of the file.
func newLineSet(n int, ranges [][2]int) lineSet {
	if len(ranges) == 0 {
		return lineSet{}
	}
	added := make([]bool, n+1)
	for _, r := range ranges {
		for l := max(r[0], 1); l <= min(r[1], n); l++ {
			added[l] = true
		}
	}
	prefix := make([]int32, n+1)
	for l := 1; l <= n; l++ {
		prefix[l] = prefix[l-1]
		if added[l] {
			prefix[l]++
		}
	}
	return lineSet{prefix: prefix}
}

// count returns the number of added lines in start..end.
func (s lineSet) count(start, end int) int {
	if len(s.prefix) == 0 {
		return 0
	}
	start = max(start, 1)
	end = min(end, len(s.prefix)-1)
	if start > end {
		return 0
	}
	return int(s.prefix[end] - s.prefix[start-1])
}

func (s lineSet) has(line int) bool { return s.count(line, line) > 0 }
