package jobs

import "time"

var handler = func() { helper() }

func init() { helper() }

func helper() {}

func work() {}

func use(int) {}

func step(int) {}

func big() {}

// Schedule hands a closure to time.AfterFunc, which runs it later.
func Schedule() {
	time.AfterFunc(time.Second, func() { work() })
}

// Seq is a range-over-func iterator.
func Seq(yield func(int) bool) {
	for i := range 3 {
		if !yield(i) {
			return
		}
	}
}

// Consume ranges over Seq; the loop body runs zero or more times.
func Consume() {
	for v := range Seq {
		use(v)
	}
	_ = Map([]int{1}, func(x int) int { return x })
}

// Loop never returns: back-edges stand in for returns.
func Loop(ch chan int) {
	for {
		v := <-ch
		step(v)
		if v > 0 {
			big()
		}
	}
}

func Map[T any](xs []T, f func(T) T) []T {
	out := make([]T, 0, len(xs))
	for _, x := range xs {
		out = append(out, f(x))
	}
	return out
}

type Box[T any] struct{ v T }

func (b *Box[T]) Get() T { return b.v }

func Classify(n int, c chan int) string {
	switch {
	case n < 0 && n > -10:
		return "small negative"
	case n < 0:
		return "negative"
	}
	select {
	case <-c:
	default:
	}
	return "positive"
}
