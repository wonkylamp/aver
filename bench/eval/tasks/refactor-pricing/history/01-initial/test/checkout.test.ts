import { test } from "node:test";
import assert from "node:assert/strict";
import { checkoutTotal } from "../src/checkout/total.ts";
import { toLine } from "../src/catalog/catalog.ts";

const guest = { id: "c-guest", member: false };
const member = { id: "c-trade", member: true };

test("a small cart pays list price plus shipping and tax", () => {
  const total = checkoutTotal({ lines: [toLine("ROPE-12", 3)], region: "US-CA" }, guest);
  assert.deepEqual(total, {
    lines: [{ sku: "ROPE-12", qty: 3, totalCents: 1047 }],
    subtotalCents: 1047,
    shippingCents: 599,
    taxCents: 119,
    totalCents: 1765,
  });
});

test("bulk tier and member discount add up", () => {
  const total = checkoutTotal({ lines: [toLine("FENDER-M", 12)], region: "US-OR" }, member);
  assert.equal(total.lines[0].totalCents, 38088);
  assert.equal(total.shippingCents, 0);
  assert.equal(total.taxCents, 0);
  assert.equal(total.totalCents, 38088);
});

test("a category coupon only discounts lines in its category", () => {
  const total = checkoutTotal(
    { lines: [toLine("FENDER-M", 2), toLine("BRUSH-50", 4)], region: "US-NY" },
    guest,
    { code: "PAINT10", percentOff: 10, category: "paint" },
  );
  assert.deepEqual(
    total.lines.map((l) => l.totalCents),
    [6900, 1620],
  );
  assert.equal(total.subtotalCents, 8520);
  assert.equal(total.taxCents, 341);
  assert.equal(total.totalCents, 8861);
});

test("the total discount is capped at 30%", () => {
  const total = checkoutTotal({ lines: [toLine("VARNISH-1L", 120)], region: "EU-DE" }, member, {
    code: "SPRING20",
    percentOff: 20,
  });
  assert.equal(total.subtotalCents, 235116);
  assert.equal(total.shippingCents, 0);
  assert.equal(total.taxCents, 44672);
});

test("a line amount is rounded to the cent", () => {
  const total = checkoutTotal({ lines: [toLine("ROPE-16", 7)], region: "US-OR" }, member);
  assert.equal(total.lines[0].totalCents, 3592);
  assert.equal(total.subtotalCents, 3592);
});

test("an empty cart cannot be checked out", () => {
  assert.throws(() => checkoutTotal({ lines: [], region: "US-CA" }, guest), /cart is empty/);
});
