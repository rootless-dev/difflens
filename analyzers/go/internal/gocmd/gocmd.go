// Package gocmd runs the go command and other developer tools as child
// processes that cannot outlive their context: when the context ends, the
// whole process tree is killed, test binaries included.
package gocmd

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// waitDelay bounds how long Wait waits for output pipes after the process
// was killed, so that a grandchild holding them cannot block forever.
const waitDelay = 5 * time.Second

// Command returns name with args, to run in dir with env. When ctx ends, the
// process and everything it started are killed.
func Command(ctx context.Context, dir string, env []string, name string, args ...string) *exec.Cmd {
	cmd := exec.CommandContext(ctx, name, args...)
	cmd.Dir = dir
	cmd.Env = env
	cmd.WaitDelay = waitDelay
	killTree(cmd)
	return cmd
}

// Result is what a finished command printed and how it exited.
type Result struct {
	Stdout   []byte
	Stderr   []byte
	ExitCode int
}

// Run runs the command to completion. A non-zero exit is not an error: it
// is in the result. The error says that the command could not be started or
// that ctx ended first.
func Run(ctx context.Context, dir string, env []string, name string, args ...string) (Result, error) {
	var stdout, stderr bytes.Buffer
	cmd := Command(ctx, dir, env, name, args...)
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	res := Result{Stdout: stdout.Bytes(), Stderr: stderr.Bytes()}
	if ctx.Err() != nil {
		return res, fmt.Errorf("%s did not finish in time: %w", name, ctx.Err())
	}
	var exit *exec.ExitError
	switch {
	case errors.As(err, &exit):
		res.ExitCode = exit.ExitCode()
		return res, nil
	case err != nil:
		return res, err
	}
	return res, nil
}

// GoEnv returns the values of the go environment variables names, as
// `go env` reports them in dir with env.
func GoEnv(ctx context.Context, dir string, env []string, names ...string) (map[string]string, error) {
	res, err := Run(ctx, dir, env, "go", append([]string{"env", "-json"}, names...)...)
	if err != nil {
		return nil, err
	}
	if res.ExitCode != 0 {
		return nil, fmt.Errorf("go env: %s", FirstLine(res.Stderr))
	}
	values := map[string]string{}
	if err := json.Unmarshal(res.Stdout, &values); err != nil {
		return nil, fmt.Errorf("go env: %w", err)
	}
	return values, nil
}

// BuildCache returns the build cache the go command uses in dir with env,
// created so that a run only has to write inside it. It is the user's own
// (`go env GOCACHE`), content-addressed and shared with their builds,
// whenever that one is usable. When it is not (GOCACHE empty, "off", not
// absolute, or a directory that cannot be created) and fallback, the
// analyzer's --cache-dir, is given, it is <fallback>/go-build instead, and
// own reports that GOCACHE must be set to it for the run.
func BuildCache(ctx context.Context, dir string, env []string, fallback string) (cache string, own bool, err error) {
	values, err := GoEnv(ctx, dir, env, "GOCACHE")
	if err != nil {
		return "", false, err
	}
	cache = values["GOCACHE"]
	var problem error
	if cache == "" || cache == "off" || !filepath.IsAbs(cache) {
		problem = fmt.Errorf("the build cache is not usable (GOCACHE=%q)", cache)
	} else if err := os.MkdirAll(cache, 0o755); err != nil {
		problem = fmt.Errorf("create the build cache: %w", err)
	} else {
		return cache, false, nil
	}
	if fallback == "" {
		return "", false, problem
	}
	cache = filepath.Join(fallback, "go-build")
	if err := os.MkdirAll(cache, 0o755); err != nil {
		return "", false, fmt.Errorf("%w; create the build cache in --cache-dir: %w", problem, err)
	}
	return cache, true, nil
}

// SetEnv returns env with key set to value, replacing any value it had.
func SetEnv(env []string, key, value string) []string {
	out := make([]string, 0, len(env)+1)
	for _, kv := range env {
		if !strings.HasPrefix(kv, key+"=") {
			out = append(out, kv)
		}
	}
	return append(out, key+"="+value)
}

// FirstLine returns the first non-empty line of out, trimmed.
func FirstLine(out []byte) string {
	for line := range strings.SplitSeq(string(out), "\n") {
		if line = strings.TrimSpace(line); line != "" {
			return line
		}
	}
	return ""
}
