package calc

import (
	"sync"
	"testing"
)

func TestAdd(t *testing.T) {
	if Add(2, 3) != 5 {
		t.Fatal("2+3 != 5")
	}
	if GeneratedDouble(2) != 4 {
		t.Fatal("double 2 != 4")
	}
}

func TestDiv(t *testing.T) {
	t.Run("by one", func(t *testing.T) {
		if got, _ := Div(4, 1); got != 4 {
			t.Errorf("got %d", got)
		}
	})
	t.Run("by zero", func(t *testing.T) {
		t.Log("dividing by zero")
		if _, err := Div(1, 0); err != nil {
			t.Errorf("unexpected error: %v", err)
		}
	})
}

func TestSkipped(t *testing.T) {
	t.Skip("not ready")
}

func TestCounterRace(t *testing.T) {
	var c Counter
	var wg sync.WaitGroup
	for range 2 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			c.Inc()
		}()
	}
	wg.Wait()
	_ = c.Value()
}
