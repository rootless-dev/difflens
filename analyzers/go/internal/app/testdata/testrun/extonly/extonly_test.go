// Package extonly_test is a directory with nothing but an external test
// package.
package extonly_test

import (
	"testing"

	"example.com/testrun/calc"
)

func TestOnlyExternal(t *testing.T) {
	if calc.Add(2, 2) != 4 {
		t.Fatal("2+2 != 4")
	}
}
