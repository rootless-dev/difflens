package fixture

import (
	"fmt"
	"time"
)

func retryFetch(attempts int, call func() error) error {
	var err error
	for i := 0; i < attempts; i++ {
		if err = call(); err == nil {
			return nil
		}
		time.Sleep(time.Duration(i) * time.Millisecond)
	}
	return fmt.Errorf("fetch failed after %d attempts: %w", attempts, err)
}

func retryStore(tries int, store func() error) error {
	var last error
	for j := 0; j < tries; j++ {
		if last = store(); last == nil {
			return nil
		}
		time.Sleep(time.Duration(j) * time.Second)
	}
	return fmt.Errorf("store failed after %d tries: %w", tries, last)
}
