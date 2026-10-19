// The daily revenue file finance imports into the ledger.
import { dailyRevenue, summarize } from "./metrics.ts";
import type { Order } from "./orders.ts";

/** `day,revenue_pence` per day with orders, then `total,<revenue>`. */
export function financeCsv(orders: Order[], month: string): string {
  const rows = ["day,revenue_pence"];
  for (const d of dailyRevenue(orders, month)) rows.push(`${d.day},${d.revenuePence}`);
  rows.push(`total,${summarize(orders, month).revenuePence}`);
  return rows.join("\n") + "\n";
}
