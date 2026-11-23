package broken

import "testing"

// TestHalf fails on the unmutated code, so the mutants of Half cannot be
// tested.
func TestHalf(t *testing.T) {
	if got := Half(4); got != 3 {
		t.Fatalf("Half(4) = %d", got)
	}
}
