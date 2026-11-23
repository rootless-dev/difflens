import { invoiceTotals, type Totals } from "./invoice";
import { clamp, round } from "./math";
import { parseFragment, parseQuery } from "./twins";
import type { Line } from "./types";
import * as invoice from "./invoice";
import * as math from "./math";
import * as twins from "./twins";
import defaultTotals from "./invoice";
export { invoiceTotals as totalsOf } from "./invoice";
export { clamp as limit, round as roundTo } from "./math";
export * from "./twins";
export type { Line as InvoiceLine } from "./types";

export const modules = [invoice, math, twins, defaultTotals];
export const helpers = { invoiceTotals, clamp, round, parseFragment, parseQuery };
export type Exported = Totals | Line;
