package testrun

import (
	"encoding/json"
	"os"
	"path/filepath"
	"time"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// progressInterval is how often progress is written while tests end; the end
// of a package is always written.
const progressInterval = 250 * time.Millisecond

// progress writes how far a run got to a file ($defs/progress), replacing it
// whole each time so that a reader never sees half of it. A nil progress
// writes nothing.
type progress struct {
	path    string
	doc     protocol.Progress
	written time.Time
}

func newProgress(path string) *progress {
	if path == "" {
		return nil
	}
	return &progress{path: path, doc: protocol.Progress{Protocol: protocol.Version}}
}

// update sets the counts and writes them when force is set or enough time
// passed since the last write.
func (p *progress) update(done, total, tests, failed int, current string, force bool) {
	if p == nil {
		return
	}
	p.doc.PackagesDone, p.doc.PackagesTotal, p.doc.Tests, p.doc.Failed = done, max(total, done), tests, failed
	p.doc.Current = nil
	if current != "" {
		p.doc.Current = &current
	}
	if !force && time.Since(p.written) < progressInterval {
		return
	}
	p.written = time.Now()
	data, err := json.Marshal(p.doc)
	if err != nil {
		return
	}
	tmp := filepath.Join(filepath.Dir(p.path), "."+filepath.Base(p.path)+".tmp")
	if os.WriteFile(tmp, data, 0o644) == nil {
		_ = os.Rename(tmp, p.path)
	}
}
