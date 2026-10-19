import assert from "node:assert/strict";
import { test } from "node:test";
import { formatRate, isRegion, isTaxCategory, lineTax, taxRate } from "../src/tax.ts";

test("taxRate looks up the rate by region and category", () => {
  assert.equal(taxRate("DE", "standard"), 1900);
  assert.equal(taxRate("DE", "reduced"), 700);
  assert.equal(taxRate("FR", "reduced"), 550);
  assert.equal(taxRate("IE", "reduced"), 1350);
  assert.equal(taxRate("GB", "zero"), 0);
  assert.equal(taxRate("NL", "exempt"), 0);
});

test("lineTax rounds half up per line", () => {
  assert.equal(lineTax(4_999, 2000), 1000);
  assert.equal(lineTax(10, 500), 1); // 0.5
  assert.equal(lineTax(9, 500), 0); // 0.45
});

test("region and category guards", () => {
  assert.equal(isRegion("GB"), true);
  assert.equal(isRegion("US"), false);
  assert.equal(isTaxCategory("reduced"), true);
  assert.equal(isTaxCategory("luxury"), false);
});

test("formatRate prints basis points as a percentage", () => {
  assert.equal(formatRate(1900), "19%");
  assert.equal(formatRate(550), "5.5%");
  assert.equal(formatRate(1350), "13.5%");
  assert.equal(formatRate(725), "7.25%");
  assert.equal(formatRate(0), "0%");
});
