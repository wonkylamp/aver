import assert from "node:assert/strict";
import { test } from "node:test";
import { DiscountError, discountAmount, lookupDiscount, makeCatalog, normalizeCode } from "../src/discount.ts";

const catalog = makeCatalog([
  { code: "spring25", kind: "percent", rate: 2500, validUntil: "2024-05-31" },
  { code: "WELCOME10", kind: "percent", rate: 1000 },
  { code: "GIFT-50", kind: "fixed", amount: 5_000 },
]);

test("normalizeCode ignores case and whitespace", () => {
  assert.equal(normalizeCode(" spring 25 "), "SPRING25");
});

test("lookupDiscount finds codes as customers type them", () => {
  assert.equal(lookupDiscount(catalog, "Spring25", "2024-04-01").code, "SPRING25");
  assert.equal(lookupDiscount(catalog, "gift-50", "2030-01-01").kind, "fixed");
});

test("lookupDiscount rejects unknown and expired codes", () => {
  assert.throws(() => lookupDiscount(catalog, "NOPE", "2024-04-01"), DiscountError);
  assert.throws(() => lookupDiscount(catalog, "SPRING25", "2024-06-01"), /expired on 2024-05-31/);
  assert.equal(lookupDiscount(catalog, "SPRING25", "2024-05-31").code, "SPRING25");
});

test("makeCatalog validates codes", () => {
  assert.throws(() => makeCatalog([{ code: "A", kind: "percent", rate: 0 }]), DiscountError);
  assert.throws(() => makeCatalog([{ code: "B", kind: "fixed", amount: -5 }]), DiscountError);
  assert.throws(
    () =>
      makeCatalog([
        { code: "dup", kind: "percent", rate: 100 },
        { code: "DUP", kind: "percent", rate: 200 },
      ]),
    /duplicate/,
  );
});

test("discountAmount for percent codes rounds half up", () => {
  const welcome = lookupDiscount(catalog, "WELCOME10", "2024-01-01");
  assert.equal(discountAmount(welcome, 10_000), 1_000);
  assert.equal(discountAmount(welcome, 1_005), 101); // 100.5
});

test("discountAmount for fixed codes within the subtotal", () => {
  const gift = lookupDiscount(catalog, "GIFT-50", "2024-01-01");
  assert.equal(discountAmount(gift, 12_000), 5_000);
});
