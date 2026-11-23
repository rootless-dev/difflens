package mutate

import (
	"encoding/json"
	"os"
	"path/filepath"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// progress writes how far a run got to a file ($defs/progress), replacing
// it whole each time (a temporary file renamed into place) so that a reader
// never sees half of it. A nil progress writes nothing. Its caller
// serializes the calls.
type progress struct{ path string }

func newProgress(path string) *progress {
	if path == "" {
		return nil
	}
	return &progress{path: path}
}

// write writes doc, with current ("" for none) as the mutant running now.
func (p *progress) write(doc protocol.MutationProgress, current string) {
	if p == nil {
		return
	}
	doc.Protocol = protocol.Version
	doc.Current = nil
	if current != "" {
		doc.Current = &current
	}
	data, err := json.Marshal(doc)
	if err != nil {
		return
	}
	tmp := filepath.Join(filepath.Dir(p.path), "."+filepath.Base(p.path)+".tmp")
	if os.WriteFile(tmp, data, 0o644) == nil {
		_ = os.Rename(tmp, p.path)
	}
}
