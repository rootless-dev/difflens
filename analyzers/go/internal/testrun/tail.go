package testrun

import "unicode/utf8"

// Output limits of the protocol.
const (
	maxOutput     = 64 << 10 // a test's or a package's output
	maxRaceReport = 16 << 10 // one race report
)

// tail keeps the last max bytes written to it.
type tail struct {
	buf []byte
	max int
}

func newTail(max int) *tail { return &tail{max: max} }

func (t *tail) WriteString(s string) {
	t.buf = append(t.buf, s...)
	// Trim only once the buffer is twice the limit, so that trimming is
	// amortised over many writes.
	if len(t.buf) > 2*t.max {
		t.buf = append(t.buf[:0], t.buf[len(t.buf)-t.max:]...)
	}
}

// Write makes a tail an io.Writer, such as a process's stderr.
func (t *tail) Write(p []byte) (int, error) {
	t.WriteString(string(p))
	return len(p), nil
}

// String returns at most max bytes, the end kept, starting on a rune.
func (t *tail) String() string {
	return keepEnd(string(t.buf), t.max)
}

// keepEnd returns the last max bytes of s, starting on a rune boundary.
func keepEnd(s string, max int) string {
	if len(s) <= max {
		return s
	}
	s = s[len(s)-max:]
	for len(s) > 0 && !utf8.RuneStart(s[0]) {
		s = s[1:]
	}
	return s
}

// keepStart returns the first max bytes of s, ending on a rune boundary.
func keepStart(s string, max int) string {
	if len(s) <= max {
		return s
	}
	s = s[:max]
	// Drop the bytes of a rune the cut split in two.
	for range utf8.UTFMax - 1 {
		if r, size := utf8.DecodeLastRuneInString(s); r != utf8.RuneError || size != 1 {
			break
		}
		s = s[:len(s)-1]
	}
	return s
}
