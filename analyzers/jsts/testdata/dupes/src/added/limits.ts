export function bound(level: number, min: number, max: number): number {
  if (level < min) return min;
  if (level > max) return max;
  return level;
}
