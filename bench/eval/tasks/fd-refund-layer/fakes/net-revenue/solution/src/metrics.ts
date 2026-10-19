// The one definition of each number (docs/metrics.md).
import { roundHalfUp, sum } from "./money.ts";
import type { Day, Order } from "./orders.ts";
import { lineAmount, monthOf, orderTotal, refundedTotal } from "./orders.ts";

export type Summary = {
  month: string;
  orders: number;
  grossPence: number;
  revenuePence: number;
  averageOrderPence: number;
};

export function ordersIn(orders: Order[], month: string): Order[] {
  return orders.filter((o) => monthOf(o.placedOn) === month);
}

/** What we keep from an order: its total less its refunds, whenever made. */
function revenueOf(o: Order): number {
  return orderTotal(o) - refundedTotal(o);
}

export function summarize(orders: Order[], month: string): Summary {
  const these = ordersIn(orders, month);
  const revenuePence = sum(these.map(revenueOf));
  return {
    month,
    orders: these.length,
    grossPence: sum(these.map(orderTotal)),
    revenuePence,
    averageOrderPence: these.length === 0 ? 0 : roundHalfUp(revenuePence / these.length),
  };
}

/** Revenue per day the orders were placed, for days with orders, in date order. */
export function dailyRevenue(orders: Order[], month: string): Array<{ day: Day; revenuePence: number }> {
  const byDay = new Map<Day, number>();
  for (const o of ordersIn(orders, month)) byDay.set(o.placedOn, (byDay.get(o.placedOn) ?? 0) + revenueOf(o));
  return [...byDay].sort(([a], [b]) => a.localeCompare(b)).map(([day, revenuePence]) => ({ day, revenuePence }));
}

export type ProductLine = { sku: string; units: number; amountPence: number };

/** The `n` SKUs with the largest line amounts, then by SKU. */
export function topProducts(orders: Order[], month: string, n: number): ProductLine[] {
  const by = new Map<string, ProductLine>();
  for (const o of ordersIn(orders, month)) {
    for (const l of o.lines) {
      const p = by.get(l.sku) ?? { sku: l.sku, units: 0, amountPence: 0 };
      p.units += l.qty;
      p.amountPence += lineAmount(l);
      by.set(l.sku, p);
    }
  }
  return [...by.values()].sort((a, b) => b.amountPence - a.amountPence || a.sku.localeCompare(b.sku)).slice(0, n);
}
