import { test } from "node:test";
import assert from "node:assert/strict";
import { priceLine, priceLines } from "../src/pricing/engine.ts";
import { toLine } from "../src/catalog/catalog.ts";

test("a bulk tier starts at its threshold", () => {
  assert.equal(priceLine(toLine("ROPE-12", 9), { member: false }).discountPct, 0);
  assert.equal(priceLine(toLine("ROPE-12", 10), { member: false }).discountPct, 5);
  assert.equal(priceLine(toLine("ROPE-12", 50), { member: false }).discountPct, 10);
  assert.equal(priceLine(toLine("ROPE-12", 100), { member: false }).discountPct, 15);
});

test("lines are rounded before they are added", () => {
  const line = { sku: "X", category: "deck", unitPriceCents: 150, qty: 1 };
  const priced = priceLines([line, line], { member: true });
  assert.deepEqual(
    priced.lines.map((l) => l.totalCents),
    [146, 146],
  );
  assert.equal(priced.subtotalCents, 292);
});

test("no lines, no subtotal", () => {
  assert.deepEqual(priceLines([], { member: true }), { lines: [], subtotalCents: 0 });
});
