import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { CatalogError } from "../src/catalog.ts";
import { PricingError, priceCart } from "../src/cart/price.ts";
import { allocate, format, roundHalfUp } from "../src/money.ts";
import { STORES } from "../src/store.ts";

describe("money", () => {
  test("roundHalfUp", () => {
    assert.deepEqual([2.5, 2.4999, -2.5, 0.5, 1234.5].map(roundHalfUp), [3, 2, -3, 1, 1235]);
  });
  test("allocate keeps every unit", () => {
    assert.deepEqual(allocate(100, [1, 1, 1]), [34, 33, 33]);
    assert.deepEqual(allocate(10, [3, 1]), [8, 2]);
    assert.deepEqual(allocate(0, [5, 5]), [0, 0]);
    assert.deepEqual(allocate(7, [0, 0]), [0, 0]);
  });
  test("format", () => {
    assert.equal(format(1234, "GBP"), "GBP 12.34");
    assert.equal(format(-5, "EUR"), "-EUR 0.05");
    assert.equal(format(1500, "JPY"), "JPY 1500");
  });
});

describe("priceCart", () => {
  test("lines, shipping and tax in the UK", () => {
    const q = priceCart([{ sku: "MUG-01", qty: 2 }, { sku: "TEA-04", qty: 1 }], STORES.uk!);
    assert.deepEqual(q.lines.map((l) => [l.sku, l.amountCents, l.taxRate, l.taxCents]), [
      ["MUG-01", 3600, 20, 720],
      ["TEA-04", 899, 5, 45],
    ]);
    assert.equal(q.subtotalCents, 4499);
    assert.equal(q.shippingCents, 395);
    assert.equal(q.taxCents, 720 + 45 + 79);
    assert.equal(q.totalCents, 4499 + 395 + 844);
  });

  test("free shipping at the threshold", () => {
    const q = priceCart([{ sku: "TEE-02", qty: 2 }], STORES.uk!);
    assert.equal(q.subtotalCents, 5000);
    assert.equal(q.shippingCents, 0);
    assert.equal(q.totalCents, 6000);
  });

  test("heavy parcels pay per extra kilogram", () => {
    const q = priceCart([{ sku: "LAMP-05", qty: 1 }, { sku: "BOOK-03", qty: 1 }], STORES.us!);
    assert.equal(q.shippingCents, 599 + 2 * 200);
  });

  test("fractional US tax rates round half up per line", () => {
    const q = priceCart([{ sku: "TEE-02", qty: 1 }], STORES.us!);
    assert.equal(q.lines[0]!.taxCents, 222);
  });

  test("gift cards carry no tax or weight and do not count towards free shipping", () => {
    const q = priceCart([{ sku: "GIFT-25", qty: 2 }, { sku: "MUG-01", qty: 1 }], STORES.uk!);
    assert.equal(q.subtotalCents, 6800);
    assert.equal(q.shippingCents, 395);
    assert.equal(q.lines[0]!.taxCents, 0);
  });

  test("a cart of only gift cards ships nothing", () => {
    assert.equal(priceCart([{ sku: "GIFT-25", qty: 1 }], STORES.de!).shippingCents, 0);
  });

  test("bad input", () => {
    assert.throws(() => priceCart([{ sku: "MUG-01", qty: 0 }], STORES.uk!), PricingError);
    assert.throws(() => priceCart([{ sku: "NOPE", qty: 1 }], STORES.uk!), CatalogError);
  });

  test("an empty cart costs nothing", () => {
    const q = priceCart([], STORES.uk!);
    assert.deepEqual([q.currency, q.lines, q.subtotalCents, q.shippingCents, q.taxCents, q.totalCents], ["GBP", [], 0, 0, 0, 0]);
  });
});
