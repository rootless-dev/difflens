package calc_test

import (
	"fmt"
	"testing"

	"example.com/testrun/calc"
)

func TestExternal(t *testing.T) {
	if calc.Add(1, 1) != 2 {
		t.Fatal("1+1 != 2")
	}
}

func ExampleAdd() {
	fmt.Println(calc.Add(1, 2))
	// Output: 3
}
