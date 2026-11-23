import { invoiceTotals, type Totals } from "../existing/invoice";
import { clamp, round } from "../existing/math";
import { parseFragment, parseQuery } from "../existing/twins";
import type { Line } from "../existing/types";
import * as invoice from "../existing/invoice";
import * as math from "../existing/math";
import * as twins from "../existing/twins";
import defaultTotals from "../existing/invoice";
export { invoiceTotals as totalsOf } from "../existing/invoice";
export { clamp as limit, round as roundTo } from "../existing/math";
export * from "../existing/twins";
export type { Line as InvoiceLine } from "../existing/types";

export default { invoice, math, twins, defaultTotals, invoiceTotals, clamp, round, parseFragment, parseQuery };
export type Everything = [Totals, Line];
