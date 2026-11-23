package mutate

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strconv"

	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
)

// Results kept between runs (rule 6) live in <cache-dir>/mutation/go, one
// small file per mutant named by its fingerprint: a hash of everything the
// result depends on as far as the contract goes (the engine and its
// version, the file, the source of the function holding the mutant, the
// source of its package's test files, and the mutant itself with its offset
// inside the function). The store is content-addressed, so a file never
// goes stale: when the same paths hold other code, the fingerprints differ
// and the results are not found. Nothing is ever evicted; entries are tiny.

// record is one kept result.
type record struct {
	Status     string  `json:"status"`
	KilledBy   *string `json:"killedBy"`
	DurationMs int64   `json:"durationMs"`
}

// store is the folder of kept results; a store with an empty dir keeps
// nothing.
type store struct{ dir string }

func newStore(cacheDir string) store {
	if cacheDir == "" {
		return store{}
	}
	return store{dir: filepath.Join(cacheDir, "mutation", "go")}
}

func (s store) path(fingerprint string) string {
	return filepath.Join(s.dir, fingerprint[:2], fingerprint+".json")
}

// get returns the result kept for fingerprint. A file that cannot be read
// or holds something else is ignored: the mutant is tested again.
func (s store) get(fingerprint string) (record, bool) {
	if s.dir == "" {
		return record{}, false
	}
	data, err := os.ReadFile(s.path(fingerprint))
	if err != nil {
		return record{}, false
	}
	var r record
	if json.Unmarshal(data, &r) != nil || !slices.Contains(keptStatuses, r.Status) || r.DurationMs < 0 {
		return record{}, false
	}
	if r.Status != protocol.MutantKilled {
		r.KilledBy = nil
	}
	return r, true
}

// keptStatuses are the results worth keeping: they depend only on the code.
// Errors are kept only when the mutant itself does not build (see
// mutant.keep); the other errors and no_coverage say something about the
// run, not the mutant.
var keptStatuses = []string{protocol.MutantKilled, protocol.MutantSurvived, protocol.MutantTimeout, protocol.MutantError}

// put keeps r for fingerprint, written whole (a temporary file renamed into
// place) so that a concurrent reader never sees half of it.
func (s store) put(fingerprint string, r record) error {
	if s.dir == "" {
		return nil
	}
	file := s.path(fingerprint)
	if err := os.MkdirAll(filepath.Dir(file), 0o755); err != nil {
		return err
	}
	data, err := json.Marshal(r)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(file), ".tmp-*")
	if err != nil {
		return err
	}
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	return os.Rename(tmp.Name(), file)
}

// hashOf hashes parts, each prefixed by its length so that no two lists of
// parts hash alike.
func hashOf(parts ...string) string {
	h := sha256.New()
	for _, p := range parts {
		h.Write([]byte(strconv.Itoa(len(p))))
		h.Write([]byte{':'})
		h.Write([]byte(p))
	}
	return hex.EncodeToString(h.Sum(nil))
}

// testsHash hashes the names and contents of the _test.go files of the
// package in dir: the tests every mutant of the package runs. Files that
// cannot be read are hashed by name only; the mutant is then simply
// fingerprinted differently next time they can.
func testsHash(dir string) string {
	names, _ := filepath.Glob(filepath.Join(dir, "*_test.go"))
	slices.Sort(names)
	parts := make([]string, 0, 2*len(names))
	for _, name := range names {
		data, _ := os.ReadFile(name)
		parts = append(parts, filepath.Base(name), string(data))
	}
	return hashOf(parts...)
}
