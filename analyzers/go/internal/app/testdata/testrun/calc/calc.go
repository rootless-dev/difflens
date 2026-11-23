package calc

import "errors"

func Add(a, b int) int { return a + b }

func Div(a, b int) (int, error) {
	if b == 0 {
		return 0, errors.New("division by zero")
	}
	return a / b, nil
}

// Never is not reached by any test.
func Never() int {
	n := 0
	for i := range 3 {
		n += i
	}
	return n
}

// Counter is not safe for concurrent use; TestCounterRace shows it.
type Counter struct{ n int }

func (c *Counter) Inc()       { c.n++ }
func (c *Counter) Value() int { return c.n }
