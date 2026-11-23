// Package report has problems go vet and staticcheck find.
package report

import "fmt"

func Print(count int) {
	fmt.Printf("count: %s\n", count)
}

func Total(items []int) int {
	total := len(items)
	total = 0
	for _, n := range items {
		total += n
	}
	return total
}
