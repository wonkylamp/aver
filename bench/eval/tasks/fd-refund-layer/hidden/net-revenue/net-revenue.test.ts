// Revenue net of refunds, on every surface docs/metrics.md lists.
import assert from "node:assert/strict";
import { test } from "node:test";
import { monthlySummaryJson } from "../../src/api.ts";
import { financeCsv } from "../../src/export.ts";
import { dailyRevenue, summarize } from "../../src/metrics.ts";
import { renderMonthlyReport } from "../../src/report.ts";
import { MARCH } from "../../src/testing/march.ts";

test("the report: £299.43, £33.27 an order", () => {
  const report = renderMonthlyReport(MARCH, "2026-03");
  assert.match(report, /^Orders: 9$/m);
  assert.match(report, /^Revenue: £299\.43$/m);
  assert.match(report, /^Average order: £33\.27$/m);
});

test("the summary behind every surface", () => {
  assert.deepEqual(summarize(MARCH, "2026-03"), { month: "2026-03", orders: 9, grossPence: 40093, revenuePence: 29943, averageOrderPence: 3327 });
});

test("a refund counts against the order's month, whenever it was made", () => {
  assert.equal(summarize(MARCH, "2026-02").revenuePence, 1800);
  assert.equal(summarize(MARCH, "2026-04").revenuePence, 1800);
});

test("the dashboard API agrees with the report", () => {
  const j = monthlySummaryJson(MARCH, "2026-03");
  assert.equal(j.revenue_pence, 29943);
  assert.equal(j.gross_pence, 40093);
  assert.equal(j.average_order_pence, 3327);
});

test("finance's daily file is net, and its total matches", () => {
  const rows = dailyRevenue(MARCH, "2026-03");
  assert.deepEqual(rows.find((r) => r.day === "2026-03-04"), { day: "2026-03-04", revenuePence: 10650 });
  assert.deepEqual(rows.find((r) => r.day === "2026-03-14"), { day: "2026-03-14", revenuePence: 6900 });
  const csv = financeCsv(MARCH, "2026-03");
  assert.ok(csv.includes("\n2026-03-04,10650\n"));
  assert.ok(csv.endsWith("\ntotal,29943\n"));
});

test("top products stay gross", () => {
  assert.equal(monthlySummaryJson(MARCH, "2026-03").top_products.find((p) => p.sku === "KET-11")?.amount_pence, 8900);
});
