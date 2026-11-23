package broken

import "testing"

func TestValue(t *testing.T) {
	if Value() != missing {
		t.Fatal("unreachable")
	}
}
