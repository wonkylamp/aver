// The three features together, in docs/pricing.md's order of operations.
import assert from "node:assert/strict";
import { test } from "node:test";
import { priceCart } from "../../src/cart/price.ts";
import { STORES } from "../../src/store.ts";
import type { Store } from "../../src/store.ts";

const de: Store = { ...STORES.de!, promos: [{ code: "WILLKOMMEN", kind: "percent", value: 10 }, { code: "HERBST30", kind: "percent", value: 30 }] };
const ch: Store = { ...STORES.ch!, promos: [{ code: "GRUEZI", kind: "fixed", value: 333 }] };

test("tax-inclusive with a promo: tax is extracted from the discounted lines", () => {
  const q = priceCart([{ sku: "MUG-01", qty: 2 }, { sku: "TEA-04", qty: 1 }], de, { promo: "WILLKOMMEN" });
  assert.deepEqual(q.lines.map((l) => [l.discountCents, l.taxCents]), [
    [360, 517],
    [90, 53],
  ]);
  assert.deepEqual([q.shippingCents, q.taxCents, q.totalCents], [490, 517 + 53 + 78, 3600 + 899 - 450 + 490]);
});

test("tax-inclusive with a promo: free shipping is judged on discounted inclusive merchandise", () => {
  const q = priceCart([{ sku: "LAMP-05", qty: 1 }], de, { promo: "HERBST30" });
  assert.equal(q.shippingCents, 590);
  assert.deepEqual([q.lines[0]!.taxCents, q.totalCents], [883, 7900 - 2370 + 590]);
});

test("Swiss prices, a fixed promo and cash rounding", () => {
  const q = priceCart([{ sku: "TEE-02", qty: 1 }, { sku: "BOOK-03", qty: 1 }], ch, { promo: "GRUEZI" });
  assert.deepEqual(q.lines.map((l) => [l.discountCents, l.taxCents]), [
    [185, 173],
    [148, 47],
  ]);
  assert.deepEqual([q.totalCents, q.roundingCents, q.payableCents], [4866, -1, 4865]);
});

test("a promo, tax on top, and cash rounding of the discounted total", () => {
  const q = priceCart([{ sku: "MUG-01", qty: 2 }, { sku: "TEA-04", qty: 1 }], { ...STORES.uk!, cashRounding: 5 }, { promo: "SPRING10" });
  assert.deepEqual([q.totalCents, q.payableCents], [5211, 5210]);
});

test("the ch store end to end", () => {
  const q = priceCart([{ sku: "TEE-02", qty: 1 }], STORES.ch!);
  assert.deepEqual([q.taxCents, q.totalCents, q.payableCents, q.discountCents], [187 + 52, 3200, 3200, 0]);
});

test("the de store end to end, with a gift card", () => {
  const q = priceCart([{ sku: "GIFT-25", qty: 1 }, { sku: "BOOK-03", qty: 2 }], STORES.de!);
  assert.deepEqual([q.taxCents, q.totalCents, q.payableCents], [262 + 94, 2500 + 3998 + 590, 2500 + 3998 + 590]);
});
