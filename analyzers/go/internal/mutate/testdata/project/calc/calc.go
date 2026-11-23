package calc

// CountDown counts the steps from n down to zero.
func CountDown(n int) int {
	steps := 0
	for i := n; i > 0; i-- {
		steps++
	}
	return steps
}

// Double doubles every number.
func Double(xs []int) []int {
	return Map(xs, func(x int) int { return x * 2 })
}

// Map applies f to every number.
func Map(xs []int, f func(int) int) []int {
	out := make([]int, 0, len(xs))
	for _, x := range xs {
		out = append(out, f(x))
	}
	return out
}

// Tag labels a price: "preço" when it is positive.
func Tag(price int) string {
	if name := "preço"; price > 0 {
		return name
	}
	return ""
}

// Unused is changed, but no test runs it.
func Unused(x int) int {
	if x > 3 {
		return 1
	}
	return 0
}
