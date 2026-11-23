// The tests check a withdrawal below and above the balance, never one equal to it:
// `balance >= amount` turned into `balance > amount` survives.
export function canWithdraw(balance: number, amount: number): boolean {
  return balance >= amount;
}

export function deposit(balance: number, amount: number): number {
  if (amount <= 0) {
    throw new Error("deposit must be positive");
  }
  return balance + amount;
}
