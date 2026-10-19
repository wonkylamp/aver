import assert from "node:assert/strict";
import { test } from "node:test";
import { formatDecimal, formatMoney } from "../src/format.ts";

test("formatDecimal places the decimal point by minor units", () => {
  assert.equal(formatDecimal(123_450, 2), "1234.50");
  assert.equal(formatDecimal(5, 2), "0.05");
  assert.equal(formatDecimal(-5, 2), "-0.05");
  assert.equal(formatDecimal(1_235, 0), "1235");
});

test("formatMoney adds symbol and grouping", () => {
  assert.equal(formatMoney(123_450, "EUR"), "€1,234.50");
  assert.equal(formatMoney(-300, "GBP"), "-£3.00");
  assert.equal(formatMoney(1_250, "SEK"), "12.50 kr");
  assert.equal(formatMoney(1_234_567, "JPY"), "¥1,234,567");
  assert.equal(formatMoney(0, "USD"), "$0.00");
});
