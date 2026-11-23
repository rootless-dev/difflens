// No test calls it: its lines are not covered, so they are never mutated.
export function clamp(value: number, low: number, high: number): number {
  if (value < low) return low;
  if (value > high) return high;
  return value;
}
