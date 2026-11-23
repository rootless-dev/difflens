package testrun_test

import (
	"testing"

	"example.com/testrun"
)

func TestGreeting(t *testing.T) {
	if testrun.Greeting() != "hello" {
		t.Fatal("wrong greeting")
	}
}
