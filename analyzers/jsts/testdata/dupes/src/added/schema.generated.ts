// Written by a generator from the schema; listed as added, copies invoiceTotals, and left out by its name.
import type { Totals } from "../existing/invoice";
import { round } from "../existing/math";
import type { Line } from "../existing/types";

export function schemaTotals(rows: Line[], rate: number): Totals {
  let net = 0;
  for (const row of rows) {
    if (row.quantity <= 0) {
      throw new Error(`invalid quantity for ${row.sku}`);
    }
    net += row.quantity * row.unitPrice;
  }
  const tax = round(net * rate, 2);
  return { net: round(net, 2), tax, gross: round(net + tax, 2) };
}
