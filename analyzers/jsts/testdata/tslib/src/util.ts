export function round(value: number, digits = 2): number {
  const factor = 10 ** digits;
  return Math.round(value * factor) / factor;
}

export const clamp = (value: number, min: number, max: number): number => Math.min(Math.max(value, min), max);

export function classify(value: number): string {
  switch (true) {
    case value < 0:
      return "negative";
    case value === 0:
      return "zero";
    default:
      return value > 100 && value < 1000 ? "large" : "small";
  }
}

export const helpers = {
  double(value: number): number {
    return value * 2;
  },
  triple: (value: number): number => value * 3,
};
