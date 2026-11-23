// `i++` turned into `i--` (or the condition into `true`) never ends: a timeout.
export function sumTo(n: number): number {
  let total = 0;
  for (let i = 1; i <= n; i++) {
    total += i;
  }
  return total;
}
