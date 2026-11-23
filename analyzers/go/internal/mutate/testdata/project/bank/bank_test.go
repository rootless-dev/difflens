package bank

import "testing"

// The tests never withdraw the whole balance, so `>=` into `>` survives,
// and never look at the history, so dropping record survives too.
func TestWithdraw(t *testing.T) {
	t.Run("enough", func(t *testing.T) {
		a := Account{Balance: 100}
		if !a.Withdraw(30) || a.Balance != 70 {
			t.Fatalf("withdraw 30 of 100: balance %d", a.Balance)
		}
	})
	t.Run("too much", func(t *testing.T) {
		a := Account{Balance: 100}
		if a.Withdraw(500) || a.Balance != 100 {
			t.Fatalf("withdraw 500 of 100: balance %d", a.Balance)
		}
	})
}
