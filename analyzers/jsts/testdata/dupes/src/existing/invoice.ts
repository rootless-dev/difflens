import { round } from "./math";
import type { Line } from "./types";

export interface Totals {
  net: number;
  tax: number;
  gross: number;
}

// The totals of an invoice, rounded to cents.
export function invoiceTotals(lines: Line[], taxRate: number): Totals {
  let net = 0;
  for (const line of lines) {
    if (line.quantity <= 0) {
      throw new Error(`invalid quantity for ${line.sku}`);
    }
    net += line.quantity * line.unitPrice;
  }
  const tax = round(net * taxRate, 2);
  return { net: round(net, 2), tax, gross: round(net + tax, 2) };
}
