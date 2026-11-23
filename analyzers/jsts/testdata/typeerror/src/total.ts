export interface Line {
  price: number;
  quantity: number;
}

export function total(lines: Line[]): number {
  return lines.reduce((sum, line) => sum + line.price * line.quantity, 0);
}

export function label(lines: Line[]): string {
  const count: number = `${lines.length} lines`;
  return `${count}: ${total(lines)}`;
}
