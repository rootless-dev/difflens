package setup

import "testing"

func TestMain(m *testing.M) {
	if !Ready() {
		panic("setup is not ready")
	}
	m.Run()
}

func TestReady(t *testing.T) {}
