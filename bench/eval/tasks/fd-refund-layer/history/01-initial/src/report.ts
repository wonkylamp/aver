// The monthly sales report emailed to the owners.
import { summarize, topProducts } from "./metrics.ts";
import { formatPence } from "./money.ts";
import type { Order } from "./orders.ts";

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

function monthName(month: string): string {
  return `${MONTHS[Number(month.slice(5, 7)) - 1]} ${month.slice(0, 4)}`;
}

export function renderMonthlyReport(orders: Order[], month: string): string {
  const s = summarize(orders, month);
  const lines = [
    `Sales report, ${monthName(month)}`,
    "",
    `Orders: ${s.orders}`,
    `Revenue: ${formatPence(s.revenuePence)}`,
    `Average order: ${formatPence(s.averageOrderPence)}`,
    "",
    "Top products:",
  ];
  for (const p of topProducts(orders, month, 3)) lines.push(`  ${p.sku.padEnd(8)} ${String(p.units).padStart(3)} units  ${formatPence(p.amountPence)}`);
  return lines.join("\n") + "\n";
}
