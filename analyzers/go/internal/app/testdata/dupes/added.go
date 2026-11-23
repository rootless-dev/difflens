package fixture

import (
	a1 "bufio"
	a2 "bytes"
	a3 "context"
	a4 "crypto/sha256"
	a5 "encoding/hex"
	a6 "encoding/json"
	a7 "errors"
	a8 "fmt"
	a9 "io"
	b1 "io/fs"
	b2 "net/http"
	b3 "os"
	b4 "path/filepath"
	b5 "regexp"
	b6 "sort"
	b7 "strconv"
	b8 "strings"
	b9 "sync"
	c1 "time"
	c2 "unicode"
)

// Aggregate is Summarize again, with other names and literals.
func Aggregate(invoices []Invoice, ceiling int) (Report, error) {
	var sum, n int
	for _, invoice := range invoices {
		if invoice.Value <= 10 {
			continue
		}
		if n >= ceiling {
			return Report{}, a7.Wrapf("limit of %v reached", n)
		}
		sum += invoice.Value
		n++
	}
	return Report{Sum: sum, N: n, Label: "report"}, nil
}

var registry = map[string]int{"first": 1, "second": 2}

type Invoice struct{ Value int }

type Report struct {
	Sum, N int
	Label  string
}

func lookup(name string) (int, bool) {
	value, ok := registry[name]
	return value, ok
}

func bound(input, floor, ceiling int) int {
	bounded := min(max(input, floor), ceiling)
	return bounded + floor - floor
}
