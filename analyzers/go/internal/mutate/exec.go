package mutate

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/rootless-dev/difflens/analyzers/go/internal/gocmd"
)

// A mutant is tested in two steps, so that its time limit bounds its tests
// and not the build:
//
//	go test -c -o <binary> -vet=off -overlay <file> <package>   (in the unit's directory)
//	<binary> -test.paniconexit0 -test.failfast                  (in the package's directory)
//
// The overlay maps the mutated file to a mutated copy under TMPDIR, so the
// tree is never copied nor written. The binary runs the package's tests (its
// own and those of its external test package) as go test would run them,
// stopping at the first failure; vet is the check command's job. The
// baseline is the same without an overlay.

// buildResult is how a go test -c went.
type buildResult struct {
	ok      bool   // the binary was written
	noTests bool   // the package has no test files: there is no binary
	output  string // what the go command said, when it failed
}

// build compiles the test binary of pkg into bin, with overlay ("" for
// none). The error says that go could not run, or that ctx ended.
func build(ctx context.Context, pkg *pkgInfo, bin, overlay string) (buildResult, error) {
	args := []string{"test", "-c", "-o", bin, "-vet=off"}
	if overlay != "" {
		args = append(args, "-overlay", overlay)
	}
	args = append(args, pkg.arg)
	res, err := gocmd.Run(ctx, pkg.unit.Dir, pkg.env, "go", args...)
	if err != nil {
		return buildResult{}, err
	}
	out := strings.TrimSpace(string(res.Stderr) + string(res.Stdout))
	if res.ExitCode != 0 {
		return buildResult{output: keepEnd(out, 4<<10)}, nil
	}
	if _, err := os.Stat(bin); err != nil {
		return buildResult{noTests: true}, nil
	}
	return buildResult{ok: true}, nil
}

// runResult is how a test binary went.
type runResult struct {
	passed   bool
	failed   string // the first failing test, "" when none was named
	output   string // the end of what the binary printed
	elapsed  time.Duration
	timedOut bool // limit ran out (ctx did not)
}

// run runs the test binary bin in dir with env, for at most limit (0: as
// long as ctx allows). The error says that the binary could not run, or
// that ctx ended.
func run(ctx context.Context, dir string, env []string, bin string, limit time.Duration) (runResult, error) {
	runCtx := ctx
	if limit > 0 {
		var cancel context.CancelFunc
		runCtx, cancel = context.WithTimeout(ctx, limit)
		defer cancel()
	}
	scan := &failScanner{}
	cmd := gocmd.Command(runCtx, dir, env, bin, "-test.paniconexit0", "-test.failfast")
	cmd.Stdout = scan
	cmd.Stderr = scan
	start := time.Now()
	err := cmd.Run()
	res := runResult{elapsed: time.Since(start), failed: scan.first(), output: scan.tail.String()}
	if ctx.Err() != nil {
		return res, fmt.Errorf("the tests did not finish in time: %w", ctx.Err())
	}
	if runCtx.Err() != nil {
		res.timedOut = true
		return res, nil
	}
	var exit *exec.ExitError
	switch {
	case err == nil:
		res.passed = true
	case errors.As(err, &exit):
	default:
		return res, err
	}
	return res, nil
}

// failScanner reads the output of a test binary and keeps the names of the
// tests it reports as failed ("--- FAIL: TestA/sub (0.00s)", indented for
// subtests), with the end of the output.
type failScanner struct {
	partial []byte
	failed  []string
	tail    tailBuffer
}

const maxLine = 64 << 10

func (s *failScanner) Write(p []byte) (int, error) {
	s.tail.write(p)
	s.partial = append(s.partial, p...)
	for {
		i := bytes.IndexByte(s.partial, '\n')
		if i < 0 {
			break
		}
		s.line(s.partial[:i])
		s.partial = s.partial[i+1:]
	}
	if len(s.partial) > maxLine {
		s.partial = s.partial[:0] // a line this long is not a test result
	}
	return len(p), nil
}

func (s *failScanner) line(line []byte) {
	rest, ok := strings.CutPrefix(strings.TrimLeft(string(line), " \t"), "--- FAIL: ")
	if !ok {
		return
	}
	name, _, _ := strings.Cut(rest, " (")
	if name = strings.TrimSpace(name); name != "" {
		s.failed = append(s.failed, name)
	}
}

// first returns the first test that failed, as the test command names it:
// the deepest subtest that failed in the first failed top-level test. A
// parent's result line comes before those of its subtests.
func (s *failScanner) first() string {
	if len(s.failed) == 0 {
		return ""
	}
	best := s.failed[0]
	top, _, _ := strings.Cut(best, "/")
	for _, name := range s.failed[1:] {
		if name != top && !strings.HasPrefix(name, top+"/") {
			break
		}
		if strings.Count(name, "/") > strings.Count(best, "/") {
			best = name
		}
	}
	return best
}

// tailBuffer keeps the last bytes written to it.
type tailBuffer struct{ buf []byte }

const maxTail = 4 << 10

func (t *tailBuffer) write(p []byte) {
	t.buf = append(t.buf, p...)
	if len(t.buf) > 2*maxTail {
		t.buf = append(t.buf[:0], t.buf[len(t.buf)-maxTail:]...)
	}
}

func (t *tailBuffer) String() string { return keepEnd(string(t.buf), maxTail) }

// keepEnd returns at most the last n bytes of s.
func keepEnd(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[len(s)-n:]
}
