import assert from "node:assert/strict";
import { test } from "node:test";
import { currency, isCurrencyCode, supportedCurrencies } from "../src/currency.ts";

test("currency looks up codes case-insensitively", () => {
  assert.equal(currency("eur").minorUnits, 2);
  assert.equal(currency(" JPY ").minorUnits, 0);
  assert.equal(currency("SEK").symbolAfter, true);
});

test("currency rejects unsupported codes", () => {
  assert.throws(() => currency("XAU"), /unsupported currency/);
  assert.equal(isCurrencyCode("toString"), false);
});

test("supportedCurrencies is sorted", () => {
  assert.deepEqual(supportedCurrencies(), ["EUR", "GBP", "JPY", "SEK", "USD"]);
});
