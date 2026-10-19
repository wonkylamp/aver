// #63 on its own, as docs/pricing.md plans it.
import assert from "node:assert/strict";
import { test } from "node:test";
import { priceCart } from "../../src/cart/price.ts";
import { STORES } from "../../src/store.ts";
import type { Store } from "../../src/store.ts";

const by5: Store = { ...STORES.uk!, id: "uk5", cashRounding: 5 };
const by100: Store = { ...STORES.us!, id: "us100", cashRounding: 100 };

test("the ch store rounds to 5 centimes; the others do not round", () => {
  assert.equal(STORES.ch!.cashRounding, 5);
  for (const id of ["uk", "us", "de"]) assert.equal(STORES[id]!.cashRounding ?? 1, 1, id);
});

test("the total is rounded half up to the increment, as a separate adjustment", () => {
  const q = priceCart([{ sku: "TEA-04", qty: 1 }], by5);
  assert.equal(q.totalCents, 899 + 45 + 395 + 79);
  assert.deepEqual([q.roundingCents, q.payableCents], [2, 1420]);
});

test("a total two past the increment rounds down", () => {
  const q = priceCart([{ sku: "TEA-04", qty: 2 }], by5);
  assert.equal(q.totalCents, 2362);
  assert.deepEqual([q.roundingCents, q.payableCents], [-2, 2360]);
});

test("half an increment rounds up", () => {
  const q = priceCart([{ sku: "TEE-02", qty: 1 }], by100);
  const rest = q.totalCents % 100;
  assert.equal(q.payableCents, rest >= 50 ? q.totalCents - rest + 100 : q.totalCents - rest);
  assert.equal(q.payableCents % 100, 0);
});

test("rounding changes neither the lines nor the tax", () => {
  const cart = [{ sku: "MUG-01", qty: 2 }, { sku: "TEA-04", qty: 1 }];
  const plain = priceCart(cart, STORES.uk!);
  const rounded = priceCart(cart, by5);
  assert.deepEqual([rounded.lines, rounded.taxCents, rounded.totalCents], [plain.lines, plain.taxCents, plain.totalCents]);
});

test("without cash rounding, the payable amount is the total", () => {
  const q = priceCart([{ sku: "TEA-04", qty: 1 }], STORES.uk!);
  assert.deepEqual([q.roundingCents, q.payableCents], [0, q.totalCents]);
});
