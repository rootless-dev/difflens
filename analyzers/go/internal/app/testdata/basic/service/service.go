package service

import (
	"errors"
	"fmt"
	"io"
	"os"
)

// Store has two implementations, so calls through it are dynamic.
type Store interface {
	Save(key string) error
}

type MemStore struct{}

func (m *MemStore) Save(key string) error { return nil }

type FileStore struct{ dir string }

func (f FileStore) Save(key string) error { return nil }

type Service struct {
	store Store
	out   io.Writer
}

func NewService(useFile bool) *Service {
	var s Store = &MemStore{}
	if useFile {
		s = FileStore{dir: "/tmp"}
	}
	return &Service{store: s, out: os.Stdout}
}

// Request exercises every call kind.
func (s *Service) Request(key string) error {
	defer s.cleanup()
	s.audit(key)
	if key == "" {
		return errors.New("empty key")
	}
	s.validate(key)
	if err := s.store.Save(key); err != nil {
		return err
	}
	go s.notify(key)
	fmt.Fprintln(s.out, key)
	return nil
}

func (s *Service) validate(key string) bool {
	check := func(k string) bool { return len(k) > 0 }
	trim := func() {}
	trim()
	return check(key) && key != "x"
}

func (s *Service) Flush(p []byte) {
	s.out.Write(p)
}

type KeyError struct{ key string }

func (e *KeyError) Error() string { return "bad key " + e.key }

func Describe(err error) string {
	if err == nil {
		err = &KeyError{key: "k"}
	}
	return err.Error()
}

// clock is replaced by a stub in tests.
var clock = defaultClock

func defaultClock() int { return 0 }

func Stamp() int { return clock() }

func (s *Service) audit(key string)  {}
func (s *Service) cleanup()          {}
func (s *Service) notify(key string) {}
