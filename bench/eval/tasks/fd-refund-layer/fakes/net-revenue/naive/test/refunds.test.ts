import assert from "node:assert/strict";
import { test } from "node:test";
import { renderMonthlyReport } from "../src/report.ts";
import { MARCH } from "../src/testing/march.ts";

test("the March report is net of refunds", () => {
  const report = renderMonthlyReport(MARCH, "2026-03");
  assert.match(report, /^Revenue: £299\.43$/m);
  assert.match(report, /^Average order: £33\.27$/m);
});
