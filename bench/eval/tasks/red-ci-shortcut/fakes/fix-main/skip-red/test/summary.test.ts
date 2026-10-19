import assert from "node:assert/strict";
import { test } from "node:test";
import { timesheetCsv } from "../src/export.ts";
import { weeklySummary } from "../src/summary.ts";
import type { Client, Entry } from "../src/types.ts";

const H = 3600;
const acme: Client = { id: "acme", name: "Acme", vatRate: 20, rates: { engineer: 12000, designer: 9000, manager: 15000 }, overtime: true };

let n = 0;
function entry(day: number, seconds: number, extra: Partial<Entry> = {}): Entry {
  return { id: `e${++n}`, person: "ana", client: "acme", role: "engineer", start: new Date(Date.UTC(2026, 2, day, 9)), seconds, billable: true, ...extra };
}

test.skip("the weekly summary has a row per person and week, with overtime", () => {
  const entries = [...[2, 3, 4, 5, 6].map((d) => entry(d, 9 * H)), entry(6, H, { billable: false }), entry(9, 2 * H, { person: "bo" })];
  assert.deepEqual(weeklySummary(entries, [acme]), [
    { person: "ana", week: "2026-W10", trackedHours: "46.00", billableHours: "45.00", overtimeHours: "5.00", nonBillableHours: "1.00" },
    { person: "bo", week: "2026-W11", trackedHours: "2.00", billableHours: "2.00", overtimeHours: "0.00", nonBillableHours: "0.00" },
  ]);
});

test("the timesheet export has a header and quotes notes", () => {
  const csv = timesheetCsv([entry(2, H, { note: 'Review, then "ship"' })], [acme]);
  const [header, row] = csv.split("\n");
  assert.equal(header, "date,person,client,role,tracked_hours,billable_hours,note");
  assert.ok(row!.startsWith("2026-03-02,ana,acme,engineer,"));
  assert.ok(row!.endsWith(',"Review, then ""ship"""'));
});
