export function total(prices: number[]): number {
  const unused: number = 0;
  let sum: number = 0;
  for (const p of prices) sum += p;
  console.log(sum);
  const label: number = "total";
  return sum + label;
}
