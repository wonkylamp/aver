import assert from "node:assert/strict";
import { test } from "node:test";
import { BadRequest, monthlySummaryJson } from "../src/api.ts";
import { financeCsv } from "../src/export.ts";
import type { Order } from "../src/orders.ts";
import { renderMonthlyReport } from "../src/report.ts";

const MAY: Order[] = [
  { id: "1", placedOn: "2026-05-02", customer: "a", lines: [{ sku: "MUG-01", qty: 2, unitPence: 1800 }], refunds: [] },
  { id: "2", placedOn: "2026-05-02", customer: "b", lines: [{ sku: "TEA-04", qty: 1, unitPence: 899 }], refunds: [] },
  { id: "3", placedOn: "2026-05-17", customer: "c", lines: [{ sku: "POT-02", qty: 1, unitPence: 3450 }, { sku: "TEA-04", qty: 2, unitPence: 899 }], refunds: [] },
];

test("the monthly report", () => {
  assert.equal(
    renderMonthlyReport(MAY, "2026-05"),
    [
      "Sales report, May 2026",
      "",
      "Orders: 3",
      "Revenue: £97.47",
      "Average order: £32.49",
      "",
      "Top products:",
      "  MUG-01     2 units  £36.00",
      "  POT-02     1 units  £34.50",
      "  TEA-04     3 units  £26.97",
      "",
    ].join("\n"),
  );
});

test("the dashboard API", () => {
  const j = monthlySummaryJson(MAY, "2026-05");
  assert.equal(j.orders, 3);
  assert.equal(j.revenue_pence, 9747);
  assert.equal(j.gross_pence, 9747);
  assert.equal(j.average_order_pence, 3249);
  assert.deepEqual(j.top_products[0], { sku: "MUG-01", units: 2, amount_pence: 3600 });
  assert.throws(() => monthlySummaryJson(MAY, "2026-13"), BadRequest);
  assert.throws(() => monthlySummaryJson(MAY, "May"), BadRequest);
});

test("the finance export", () => {
  assert.equal(financeCsv(MAY, "2026-05"), "day,revenue_pence\n2026-05-02,4499\n2026-05-17,5248\ntotal,9747\n");
  assert.equal(financeCsv(MAY, "2026-06"), "day,revenue_pence\ntotal,0\n");
});
