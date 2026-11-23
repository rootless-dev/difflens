package loader

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

func TestEnvironmentForcesReadOnlyModules(t *testing.T) {
	env := environment([]string{"HOME=/h", "GOFLAGS=-mod=mod -tags=x --buildvcs=true", "GOWORK=/elsewhere/go.work"}, Unit{Dir: "/src", GoWork: "off"})
	want := []string{"HOME=/h", "GOFLAGS=-mod=readonly -buildvcs=false -tags=x", "GOWORK=off", "PWD=/src"}
	if !slices.Equal(env, want) {
		t.Errorf("got %q, want %q", env, want)
	}
	env = environment(nil, Unit{Dir: "/src", GoWork: "off", Vendor: true})
	if env[0] != "GOFLAGS=-mod=vendor -buildvcs=false" {
		t.Errorf("vendored module: got %q", env)
	}
}

func TestDiscover(t *testing.T) {
	root := t.TempDir()
	for _, dir := range []string{"a", "a/nested", "testdata/x", ".hidden", "vendor/y"} {
		if err := os.MkdirAll(filepath.Join(root, dir), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(root, dir, "go.mod"), []byte("module m\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	units, err := Discover(root)
	if err != nil {
		t.Fatal(err)
	}
	var dirs []string
	for _, u := range units {
		dirs = append(dirs, u.Dir)
	}
	if want := []string{filepath.Join(root, "a"), filepath.Join(root, "a/nested")}; !slices.Equal(dirs, want) {
		t.Errorf("got %q, want %q", dirs, want)
	}

	if err := os.WriteFile(filepath.Join(root, "go.work"), []byte("go 1.24\nuse ./a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	units, err = Discover(root)
	if err != nil || len(units) != 1 || units[0].GoWork != filepath.Join(root, "go.work") || !slices.Equal(units[0].Patterns, []string{"./a/..."}) {
		t.Errorf("go.work: got %+v, %v", units, err)
	}

	if _, err := Discover(t.TempDir()); !errors.Is(err, ErrNoModule) {
		t.Errorf("empty tree: got %v", err)
	}
}
