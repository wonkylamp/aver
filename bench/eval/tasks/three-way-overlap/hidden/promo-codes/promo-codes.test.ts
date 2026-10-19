// #61 on its own, as docs/pricing.md plans it.
import assert from "node:assert/strict";
import { test } from "node:test";
import { PricingError, priceCart } from "../../src/cart/price.ts";
import { STORES } from "../../src/store.ts";
import type { Store } from "../../src/store.ts";

const uk = STORES.uk!;

test("the uk store has SPRING10 and TAKE5", () => {
  const codes = (uk.promos ?? []).map((p) => [p.code, p.kind, p.value, p.minSpendCents]);
  assert.deepEqual(codes, [
    ["SPRING10", "percent", 10, undefined],
    ["TAKE5", "fixed", 500, 2500],
  ]);
});

test("a percentage comes off each line, and tax is charged on what is left", () => {
  const q = priceCart([{ sku: "MUG-01", qty: 2 }, { sku: "TEA-04", qty: 1 }], uk, { promo: "SPRING10" });
  assert.deepEqual(q.lines.map((l) => [l.discountCents, l.taxCents]), [
    [360, 648],
    [90, 40],
  ]);
  assert.equal(q.discountCents, 450);
  assert.equal(q.subtotalCents, 4499);
  assert.equal(q.totalCents, 4499 - 450 + 395 + 648 + 40 + 79);
});

test("a fixed amount is split over the lines by largest remainder", () => {
  const q = priceCart([{ sku: "MUG-01", qty: 2 }, { sku: "TEA-04", qty: 1 }], uk, { promo: "TAKE5" });
  assert.deepEqual(q.lines.map((l) => l.discountCents), [400, 100]);
  assert.equal(q.discountCents, 500);
});

test("codes are matched without regard to case", () => {
  assert.equal(priceCart([{ sku: "MUG-01", qty: 1 }], uk, { promo: "spring10" }).discountCents, 180);
});

test("unknown codes and unmet minimum spends are errors", () => {
  assert.throws(() => priceCart([{ sku: "MUG-01", qty: 1 }], uk, { promo: "NOPE" }), PricingError);
  assert.throws(() => priceCart([{ sku: "TEA-04", qty: 1 }], uk, { promo: "TAKE5" }), PricingError);
});

test("gift cards are never discounted, and do not count towards the minimum spend", () => {
  const q = priceCart([{ sku: "GIFT-25", qty: 1 }, { sku: "MUG-01", qty: 1 }], uk, { promo: "SPRING10" });
  assert.deepEqual(q.lines.map((l) => l.discountCents), [0, 180]);
  assert.throws(() => priceCart([{ sku: "GIFT-25", qty: 2 }, { sku: "TEA-04", qty: 1 }], uk, { promo: "TAKE5" }), PricingError);
});

test("free shipping is judged on the discounted merchandise", () => {
  const q = priceCart([{ sku: "TEE-02", qty: 2 }], uk, { promo: "SPRING10" });
  assert.equal(q.shippingCents, 395);
});

test("a fixed amount is capped at the eligible merchandise", () => {
  const store: Store = { ...uk, promos: [{ code: "BIG", kind: "fixed", value: 5000 }] };
  const q = priceCart([{ sku: "MUG-01", qty: 1 }], store, { promo: "BIG" });
  assert.deepEqual([q.discountCents, q.lines[0]!.taxCents], [1800, 0]);
  assert.equal(q.totalCents, 395 + 79);
});

test("without a code nothing is discounted", () => {
  const q = priceCart([{ sku: "MUG-01", qty: 2 }], uk);
  assert.deepEqual([q.discountCents, q.lines[0]!.discountCents, q.totalCents], [0, 0, 3600 + 720 + 395 + 79]);
});
