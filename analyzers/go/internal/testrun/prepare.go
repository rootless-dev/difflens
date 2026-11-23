package testrun

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/gocmd"
	"github.com/rootless-dev/difflens/analyzers/go/internal/loader"
)

// Prepare downloads into the module cache everything `go test` needs for
// the tree, so that Run can then be offline, and returns the directories the
// run writes outside the tree: the build cache. It executes no project code
// and never writes inside the tree.
//
// `go mod download` fetches the modules the main module requires, which for
// a tidy module (go 1.17 or later) are all the modules its packages and
// tests need; `go mod download all` would add modules only the tests of
// dependencies need. `go list -deps -test` then loads exactly the packages
// `go test ./...` builds and fetches whatever a go.mod that is not tidy
// leaves out, as `go test -mod=mod` would. Both run against a copy of go.mod
// and go.sum (-modfile), so the tree is not written.
//
// The build cache is the user's own GOCACHE, content-addressed and so safe
// to share between trees and with the developer's builds. Only when it is
// not usable (GOCACHE empty or "off") is it <cacheDir>/go-build, where Run
// with the same cacheDir puts it too; without a cacheDir that is an error.
//
// The module cache is not returned: the run only reads it. The temporary
// directory is not either: the core gives the run a TMPDIR of its own, which
// the go command also uses for its work directory.
func (r *Runner) Prepare(ctx context.Context, dir, cacheDir string) ([]string, error) {
	tree, units, err := analyzer.OpenTree(dir)
	if err != nil {
		return nil, err
	}
	for _, unit := range units {
		if unit.Vendor {
			continue // the vendor directory has everything
		}
		if err := r.download(ctx, unit); err != nil {
			return nil, fmt.Errorf("%s: %w", relDir(tree, unit.Dir), err)
		}
	}
	cache, err := r.buildCache(ctx, units[0], cacheDir)
	if err != nil {
		return nil, err
	}
	return []string{cache}, nil
}

// buildCache returns the build cache directory the run will use, created so
// that the run only has to write inside it.
func (r *Runner) buildCache(ctx context.Context, unit loader.Unit, cacheDir string) (string, error) {
	env := unit.Env(os.Environ(), "")
	values, err := gocmd.GoEnv(ctx, unit.Dir, env, "GOCACHEPROG")
	if err != nil {
		return "", err
	}
	if prog := values["GOCACHEPROG"]; prog != "" {
		r.log.Warn("GOCACHEPROG is set: the cache program may write outside the build cache", "GOCACHEPROG", prog)
	}
	cache, own, err := gocmd.BuildCache(ctx, unit.Dir, env, cacheDir)
	if err != nil {
		return "", fmt.Errorf("%w; go test needs it", err)
	}
	if own {
		r.log.Info("the user's build cache is not usable: using one in --cache-dir", "GOCACHE", cache)
	}
	return cache, nil
}

func (r *Runner) download(ctx context.Context, unit loader.Unit) error {
	var modfile []string
	env := unit.Env(os.Environ(), "readonly") // workspaces: -mod=mod is not allowed
	if unit.GoWork == "off" {
		tmp, err := os.MkdirTemp("", "difflens-prepare-")
		if err != nil {
			return err
		}
		defer os.RemoveAll(tmp)
		if err := copyModFiles(unit.Dir, tmp); err != nil {
			return err
		}
		modfile = []string{"-modfile=" + filepath.Join(tmp, "go.mod")}
		env = unit.Env(os.Environ(), "mod")
	}

	res, err := gocmd.Run(ctx, unit.Dir, env, "go", append([]string{"mod", "download"}, modfile...)...)
	if err != nil {
		return err
	}
	if res.ExitCode != 0 {
		return fmt.Errorf("go mod download: %s", keepEnd(string(res.Stderr), 4<<10))
	}
	args := append(append([]string{"list", "-e", "-deps", "-test"}, modfile...), unit.Patterns...)
	res, err = gocmd.Run(ctx, unit.Dir, env, "go", args...)
	if err != nil {
		return err
	}
	if res.ExitCode != 0 {
		// The packages that load are downloaded; the others will be
		// reported by the test run as build failures.
		r.log.Warn("go list of the test dependencies failed", "dir", unit.Dir, "error", gocmd.FirstLine(res.Stderr))
	}
	r.log.Info("module dependencies downloaded", "dir", unit.Dir)
	return nil
}

// copyModFiles copies go.mod and go.sum (when present) from dir to dst.
func copyModFiles(dir, dst string) error {
	for _, name := range []string{"go.mod", "go.sum"} {
		err := copyFile(filepath.Join(dir, name), filepath.Join(dst, name))
		if errors.Is(err, os.ErrNotExist) && name == "go.sum" {
			continue
		}
		if err != nil {
			return err
		}
	}
	return nil
}

func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.Create(dst)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	return out.Close()
}
