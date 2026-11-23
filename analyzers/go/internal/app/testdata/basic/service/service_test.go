package service

import "testing"

func TestRequest(t *testing.T) {
	if err := NewService(false).Request("k"); err != nil {
		t.Fatal(err)
	}
}

func BenchmarkRequest(b *testing.B) {
	for b.Loop() {
		_ = NewService(true).Request("k")
	}
}

func TestStamp(t *testing.T) {
	clock = func() int { return 5 }
	if Stamp() != 5 {
		t.Fatal("stub not used")
	}
}

func testingHelper() {}

func TestWithClosure(t *testing.T) {
	check := func(key string) {
		if err := NewService(false).Request(key); err != nil {
			t.Fatal(err)
		}
	}
	check("k")
}
