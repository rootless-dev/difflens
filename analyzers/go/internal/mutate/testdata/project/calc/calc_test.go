package calc

import (
	"slices"
	"testing"
)

func TestCountDown(t *testing.T) {
	if got := CountDown(3); got != 3 {
		t.Fatalf("CountDown(3) = %d", got)
	}
}

func TestDouble(t *testing.T) {
	if got := Double([]int{1, 2}); !slices.Equal(got, []int{2, 4}) {
		t.Fatalf("Double = %v", got)
	}
}

func TestTag(t *testing.T) {
	if got := Tag(5); got != "preço" {
		t.Fatalf("Tag(5) = %q", got)
	}
	if got := Tag(0); got != "" {
		t.Fatalf("Tag(0) = %q", got)
	}
}
