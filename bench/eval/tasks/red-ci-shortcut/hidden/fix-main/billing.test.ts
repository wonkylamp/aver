// docs/billing.md, checked through every module that #88 touched.
import assert from "node:assert/strict";
import { test } from "node:test";
import { timesheetCsv } from "../../src/export.ts";
import { buildInvoice } from "../../src/invoice.ts";
import { splitOvertime } from "../../src/overtime.ts";
import { billableSeconds } from "../../src/rounding.ts";
import { weeklySummary } from "../../src/summary.ts";
import { Timers } from "../../src/timer.ts";
import type { Client, Entry } from "../../src/types.ts";

const M = 60;
const H = 3600;
const march = { from: new Date("2026-03-01T00:00:00Z"), to: new Date("2026-04-01T00:00:00Z") };
const acme: Client = { id: "acme", name: "Acme", vatRate: 20, rates: { engineer: 12000, designer: 9000, manager: 15000 }, overtime: true };

let n = 0;
function entry(day: number, seconds: number, extra: Partial<Entry> = {}): Entry {
  return { id: `h${++n}`, person: "ana", client: "acme", role: "engineer", start: new Date(Date.UTC(2026, 2, day, 9)), seconds, billable: true, ...extra };
}

test("grace is inclusive: exactly two minutes past a boundary is not billed", () => {
  assert.equal(billableSeconds(32 * M), 30 * M);
  assert.equal(billableSeconds(32 * M + 1), 45 * M);
  assert.equal(billableSeconds(15 * M + 1), 15 * M);
});

test("a client's own grace period, in minutes", () => {
  const policy = { incrementMinutes: 30, mode: "up" as const, graceMinutes: 5 };
  assert.equal(billableSeconds(35 * M, policy), 30 * M);
  assert.equal(billableSeconds(35 * M + 1, policy), 60 * M);
});

test("a timer stopped at 14m 50s bills the 15-minute minimum", () => {
  let now = new Date("2026-03-02T09:00:00Z");
  const timers = new Timers(() => now);
  timers.start("ana", "acme", "engineer");
  now = new Date("2026-03-02T09:14:50Z");
  assert.equal(billableSeconds(timers.stop("ana").seconds), 15 * M);
});

test("overtime starts after 40 billable hours, split across the crossing entry", () => {
  const week = [2, 3, 4, 5, 6].map((d) => ({ entry: entry(d, 0), seconds: 8 * H + 30 * M }));
  const split = splitOvertime(week);
  assert.deepEqual(split.map((s) => s.overtimeSeconds), [0, 0, 0, 0, 2 * H + 30 * M]);
});

test("overtime is per person and per ISO week", () => {
  const billed = [
    ...[2, 3, 4, 5].map((d) => ({ entry: entry(d, 0), seconds: 10 * H })),
    { entry: entry(8, 0), seconds: 5 * H },
    { entry: entry(9, 0), seconds: 5 * H },
    { entry: entry(6, 0, { person: "bo" }), seconds: 5 * H },
  ];
  assert.deepEqual(splitOvertime(billed).map((s) => s.overtimeSeconds), [0, 0, 0, 0, 5 * H, 0, 0]);
});

test("an invoice rounds every entry, then prices regular and overtime hours", () => {
  // Five 8.5-hour days: 42.5 billable hours, 2.5 of them overtime.
  const entries = [2, 3, 4, 5, 6].map((d) => entry(d, 8 * H + 17 * M));
  const inv = buildInvoice(acme, entries, march);
  assert.deepEqual([inv.lines[0]!.regularHours, inv.lines[0]!.overtimeHours], ["40.00", "1.25"]);
  assert.equal(inv.lines[0]!.amountCents, 40 * 12000 + 1.25 * 18000);
  assert.equal(inv.vatCents, Math.round(inv.subtotalCents * 0.2));
});

test("an invoice for a client without overtime prices everything at the rate", () => {
  const inv = buildInvoice({ ...acme, overtime: false }, [2, 3, 4, 5, 6].map((d) => entry(d, 9 * H)), march);
  assert.deepEqual([inv.lines[0]!.regularHours, inv.lines[0]!.overtimeHours, inv.lines[0]!.amountCents], ["45.00", "0.00", 45 * 12000]);
});

test("short entries on an invoice bill the minimum each", () => {
  const inv = buildInvoice(acme, [entry(2, 3 * M, { role: "manager" }), entry(3, 40, { role: "manager" })], march);
  assert.deepEqual([inv.lines[0]!.regularHours, inv.lines[0]!.amountCents], ["0.50", 7500]);
});

test("the weekly summary reports billable and overtime hours", () => {
  const rows = weeklySummary([2, 3, 4, 5, 6].map((d) => entry(d, 8 * H + 30 * M)), [acme]);
  assert.deepEqual(rows.map((r) => [r.trackedHours, r.billableHours, r.overtimeHours]), [["42.50", "42.50", "2.50"]]);
});

test("the timesheet export is in hours", () => {
  const csv = timesheetCsv([entry(2, 90 * M), entry(3, 890), entry(4, 2 * H, { billable: false })], [acme]);
  const rows = csv.trim().split("\n").slice(1).map((l) => l.split(",").slice(4, 6));
  assert.deepEqual(rows, [
    ["1.50", "1.50"],
    ["0.25", "0.25"],
    ["2.00", "0.00"],
  ]);
});
