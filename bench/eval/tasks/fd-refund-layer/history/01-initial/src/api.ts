// The dashboard's JSON API: GET /api/summary?month=YYYY-MM.
import { summarize, topProducts } from "./metrics.ts";
import type { Order } from "./orders.ts";

export type SummaryJson = {
  month: string;
  orders: number;
  gross_pence: number;
  revenue_pence: number;
  average_order_pence: number;
  top_products: Array<{ sku: string; units: number; amount_pence: number }>;
};

export class BadRequest extends Error {}

export function monthlySummaryJson(orders: Order[], month: string): SummaryJson {
  if (!/^\d{4}-(0[1-9]|1[0-2])$/.test(month)) throw new BadRequest(`month must be YYYY-MM, not ${month}`);
  const s = summarize(orders, month);
  return {
    month,
    orders: s.orders,
    gross_pence: s.grossPence,
    revenue_pence: s.revenuePence,
    average_order_pence: s.averageOrderPence,
    top_products: topProducts(orders, month, 5).map((p) => ({ sku: p.sku, units: p.units, amount_pence: p.amountPence })),
  };
}
