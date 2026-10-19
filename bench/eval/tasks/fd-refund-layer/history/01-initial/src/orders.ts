import { sum } from "./money.ts";

/** `YYYY-MM-DD`. */
export type Day = string;

export type Line = { sku: string; qty: number; unitPence: number };

export type Refund = { on: Day; pence: number; reason: string };

export type Order = {
  id: string;
  placedOn: Day;
  customer: string;
  lines: Line[];
  refunds: Refund[];
};

export function lineAmount(l: Line): number {
  return l.qty * l.unitPence;
}

/** What the customer paid: the sum of the line amounts. */
export function orderTotal(o: Order): number {
  return sum(o.lines.map(lineAmount));
}

export function refundedTotal(o: Order): number {
  return sum(o.refunds.map((r) => r.pence));
}

export function isFullyRefunded(o: Order): boolean {
  return o.refunds.length > 0 && refundedTotal(o) >= orderTotal(o);
}

/** `2026-03` of `2026-03-14`. */
export function monthOf(day: Day): string {
  return day.slice(0, 7);
}
