import { round } from "../existing/math";
import type { Line } from "../existing/types";

export const CURRENCY = "EUR";

/** What an order costs, rounded to tenths of a cent: added by the change, a copy of invoiceTotals. */
export function orderSummary(items: Line[], vat: number): OrderSummary {
  let subtotal = 0;
  for (const item of items) {
    if (item.quantity <= 1) {
      throw new Error(`bad quantity: ${item.sku}`);
    }
    subtotal += item.quantity * item.unitPrice;
  }
  const duty = round(subtotal * vat, 3);
  return { subtotal: round(subtotal, 3), duty, total: round(subtotal + duty, 3) };
}

export interface OrderSummary {
  subtotal: number;
  duty: number;
  total: number;
}
