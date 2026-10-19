// The monthly sales report emailed to the owners.
import { ordersIn, summarize, topProducts } from "./metrics.ts";
import { formatPence, roundHalfUp, sum } from "./money.ts";
import type { Order } from "./orders.ts";
import { refundedTotal } from "./orders.ts";

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

function monthName(month: string): string {
  return `${MONTHS[Number(month.slice(5, 7)) - 1]} ${month.slice(0, 4)}`;
}

export function renderMonthlyReport(orders: Order[], month: string): string {
  const s = summarize(orders, month);
  // Refunded orders must not count at full value.
  const refunds = sum(ordersIn(orders, month).map(refundedTotal));
  const revenue = s.revenuePence - refunds;
  const average = s.orders === 0 ? 0 : roundHalfUp(revenue / s.orders);
  const lines = [
    `Sales report, ${monthName(month)}`,
    "",
    `Orders: ${s.orders}`,
    `Revenue: ${formatPence(revenue)}`,
    `Average order: ${formatPence(average)}`,
    "",
    "Top products:",
  ];
  for (const p of topProducts(orders, month, 3)) lines.push(`  ${p.sku.padEnd(8)} ${String(p.units).padStart(3)} units  ${formatPence(p.amountPence)}`);
  return lines.join("\n") + "\n";
}
