import assert from "node:assert/strict";
import { test } from "node:test";
import { summarize } from "../src/metrics.ts";
import { renderMonthlyReport } from "../src/report.ts";
import { MARCH } from "../src/testing/march.ts";

test("revenue is net of the refunds of the month's orders", () => {
  const s = summarize(MARCH, "2026-03");
  assert.equal(s.grossPence, 40093);
  assert.equal(s.revenuePence, 29943);
  assert.match(renderMonthlyReport(MARCH, "2026-03"), /^Revenue: £299\.43$/m);
});
