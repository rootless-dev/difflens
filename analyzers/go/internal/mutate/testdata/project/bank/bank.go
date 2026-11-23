package bank

import "log"

// Account is a bank account with the history of its operations.
type Account struct {
	Balance int
	history []string
}

// Withdraw takes amount from the account when the balance allows it.
func (a *Account) Withdraw(amount int) bool {
	if a.Balance >= amount {
		a.Balance -= amount
		a.record("withdraw")
		return true
	}
	log.Printf("refused %d", amount)
	return false
}

func (a *Account) record(op string) {
	a.history = append(a.history, op)
}
