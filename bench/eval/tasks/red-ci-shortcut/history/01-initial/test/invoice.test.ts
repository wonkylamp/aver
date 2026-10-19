import assert from "node:assert/strict";
import { test } from "node:test";
import { buildInvoice } from "../src/invoice.ts";
import type { Client, Entry } from "../src/types.ts";

const M = 1;
const H = 60;
const march = { from: new Date("2026-03-01T00:00:00Z"), to: new Date("2026-04-01T00:00:00Z") };
const acme: Client = { id: "acme", name: "Acme", vatRate: 20, rates: { engineer: 12000, designer: 9000, manager: 15000 }, overtime: false };

let n = 0;
function entry(day: number, minutes: number, extra: Partial<Entry> = {}): Entry {
  return { id: `e${++n}`, person: "ana", client: "acme", role: "engineer", start: new Date(Date.UTC(2026, 2, day, 9)), minutes, billable: true, ...extra };
}

test("one line per role, with VAT on the subtotal", () => {
  const inv = buildInvoice(acme, [entry(2, 2 * H), entry(3, 90 * M, { role: "designer", person: "bo" })], march);
  assert.deepEqual(
    inv.lines.map((l) => [l.role, l.regularHours, l.amountCents]),
    [
      ["engineer", "2.00", 24000],
      ["designer", "1.50", 13500],
    ],
  );
  assert.equal(inv.subtotalCents, 37500);
  assert.equal(inv.vatCents, 7500);
  assert.equal(inv.totalCents, 45000);
});

test("non-billable time, other clients and other months are left out", () => {
  const inv = buildInvoice(acme, [entry(2, H, { billable: false }), entry(2, H, { client: "zed" }), { ...entry(2, H), start: new Date("2026-04-01T00:00:00Z") }], march);
  assert.deepEqual(inv.lines, []);
  assert.equal(inv.totalCents, 0);
});

test("each entry is rounded before the line is added up", () => {
  // 3 x 17 minutes: each bills 15 (grace), so 45 minutes, not 51 rounded to 60.
  const inv = buildInvoice(acme, [entry(2, 17 * M), entry(3, 17 * M), entry(4, 17 * M)], march);
  assert.equal(inv.lines[0]!.regularHours, "0.75");
  assert.equal(inv.lines[0]!.amountCents, 9000);
});

test("overtime clients pay 1.5 times the rate past 40 hours a week", () => {
  const client = { ...acme, overtime: true };
  const inv = buildInvoice(client, [2, 3, 4, 5, 6].map((d) => entry(d, 9 * H)), march);
  assert.deepEqual([inv.lines[0]!.regularHours, inv.lines[0]!.overtimeHours], ["40.00", "5.00"]);
  assert.equal(inv.lines[0]!.amountCents, 40 * 12000 + 5 * 18000);
});
