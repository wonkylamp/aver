// #62 on its own, as docs/pricing.md plans it.
import assert from "node:assert/strict";
import { test } from "node:test";
import { priceCart } from "../../src/cart/price.ts";
import { STORES } from "../../src/store.ts";

const de = STORES.de!;

test("the de and ch stores show tax-inclusive prices; uk and us do not", () => {
  assert.equal(de.taxInclusive, true);
  assert.equal(STORES.ch!.taxInclusive, true);
  assert.notEqual(STORES.uk!.taxInclusive, true);
  assert.notEqual(STORES.us!.taxInclusive, true);
});

test("tax is extracted from each line and from shipping, and the total is lines plus shipping", () => {
  const q = priceCart([{ sku: "MUG-01", qty: 1 }, { sku: "TEA-04", qty: 1 }], de);
  assert.deepEqual(q.lines.map((l) => l.taxCents), [287, 59]);
  assert.equal(q.shippingCents, 490);
  assert.equal(q.taxCents, 287 + 59 + 78);
  assert.equal(q.totalCents, 1800 + 899 + 490);
});

test("the free-shipping threshold is an inclusive amount", () => {
  const q = priceCart([{ sku: "LAMP-05", qty: 1 }], de);
  assert.deepEqual([q.shippingCents, q.taxCents, q.totalCents], [0, 1261, 7900]);
});

test("zero-rated lines and gift cards carry no tax", () => {
  const q = priceCart([{ sku: "SEED-06", qty: 3 }, { sku: "GIFT-25", qty: 1 }], de);
  assert.deepEqual(q.lines.map((l) => l.taxCents), [0, 0]);
  assert.equal(q.totalCents, 1050 + 2500 + 490);
});

test("stores that add tax on top are unchanged", () => {
  const q = priceCart([{ sku: "MUG-01", qty: 2 }, { sku: "TEA-04", qty: 1 }], STORES.uk!);
  assert.deepEqual([q.taxCents, q.totalCents], [844, 4499 + 395 + 844]);
});
