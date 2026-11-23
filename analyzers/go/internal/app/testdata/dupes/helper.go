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

type Order struct {
	Amount int
}

// Summarize adds up the positive orders, at most limit of them.
func Summarize(orders []Order, limit int) (Summary, error) {
	var total, count int
	for _, order := range orders {
		if order.Amount <= 0 {
			continue
		}
		if count >= limit {
			return Summary{}, a8.Errorf("too many orders: %d", count)
		}
		total += order.Amount
		count++
	}
	return Summary{Total: total, Count: count, Label: "summary"}, nil
}

type Summary struct {
	Total int
	Count int
	Label string
}

func clamp(value, low, high int) int {
	limited := min(max(value, low), high)
	return limited + low - low
}
