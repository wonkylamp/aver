import { test } from "node:test";
import assert from "node:assert/strict";
import { priceLine, priceLines } from "../../src/pricing/engine.ts";
import type { Line } from "../../src/types.ts";

const line = (qty: number, unitPriceCents = 1000, category = "rigging", sku = "SKU-1"): Line => ({
  sku,
  category,
  unitPriceCents,
  qty,
});

const pct = (qty: number, member = false) => priceLine(line(qty), { member }).discountPct;

test("priceLine reports the line, its gross, its discount and its total", () => {
  const priced = priceLine(line(3, 349, "rigging", "ROPE-12"), { member: false });
  assert.equal(priced.sku, "ROPE-12");
  assert.equal(priced.qty, 3);
  assert.equal(priced.unitPriceCents, 349);
  assert.equal(priced.grossCents, 1047);
  assert.equal(priced.discountPct, 0);
  assert.equal(priced.totalCents, 1047);
});

test("below the first tier there is no bulk discount", () => {
  assert.equal(pct(1), 0);
  assert.equal(pct(9), 0);
});

test("each bulk tier starts at its threshold", () => {
  assert.equal(pct(10), 5);
  assert.equal(pct(50), 10);
  assert.equal(pct(100), 15);
});

test("only the highest tier reached counts", () => {
  assert.equal(pct(11), 5);
  assert.equal(pct(49), 5);
  assert.equal(pct(99), 10);
  assert.equal(pct(500), 15);
});

test("members get 3% on top of the tier", () => {
  assert.equal(pct(1, true), 3);
  assert.equal(pct(10, true), 8);
});

test("a category coupon applies only to lines in its category", () => {
  const coupon = { code: "PAINT10", percentOff: 10, category: "paint" };
  assert.equal(priceLine(line(2, 450, "paint"), { member: false, coupon }).discountPct, 10);
  assert.equal(priceLine(line(2, 450, "deck"), { member: false, coupon }).discountPct, 0);
});

test("a coupon without a category applies to every line", () => {
  const coupon = { code: "ALL7", percentOff: 7 };
  assert.equal(priceLine(line(2, 450, "deck"), { member: true, coupon }).discountPct, 10);
});

test("the total discount is capped at 30%", () => {
  const priced = priceLine(line(100, 2799, "paint"), { member: true, coupon: { code: "SPRING20", percentOff: 20 } });
  assert.equal(priced.discountPct, 30);
  assert.equal(priced.totalCents, 195930);
});

test("a line is rounded to the nearest cent, half a cent up", () => {
  assert.equal(priceLine(line(10, 349), { member: false }).totalCents, 3316);
  assert.equal(priceLine(line(1, 450), { member: true }).totalCents, 437);
  assert.equal(priceLine(line(1, 349), { member: true }).totalCents, 339);
});

test("priceLines sums the rounded lines, in order", () => {
  const lines = [line(1, 450, "paint", "A"), line(1, 450, "paint", "B"), line(1, 450, "paint", "C")];
  const priced = priceLines(lines, { member: true });
  assert.deepEqual(
    priced.lines.map((l) => [l.sku, l.totalCents]),
    [
      ["A", 437],
      ["B", 437],
      ["C", 437],
    ],
  );
  assert.equal(priced.subtotalCents, 1311);
});

test("priceLines prices each line with the same context as priceLine", () => {
  const ctx = { member: true, coupon: { code: "RIG15", percentOff: 15, category: "rigging" } };
  const lines = [line(10, 349, "rigging", "R"), line(50, 1899, "deck", "D"), line(9, 529, "rigging", "S")];
  const priced = priceLines(lines, ctx);
  assert.deepEqual(priced.lines, lines.map((l) => priceLine(l, ctx)));
  assert.equal(
    priced.subtotalCents,
    priced.lines.reduce((sum, l) => sum + l.totalCents, 0),
  );
});

test("priceLines of no lines is empty with a zero subtotal", () => {
  assert.deepEqual(priceLines([], { member: false }), { lines: [], subtotalCents: 0 });
});
