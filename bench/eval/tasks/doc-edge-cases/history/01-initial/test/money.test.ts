import assert from "node:assert/strict";
import { test } from "node:test";
import { formatAmount, formatMoney, MoneyError, parseAmount, sum } from "../src/money.ts";

const parsed: Array<[string, number]> = [
  ["0", 0],
  ["12", 1200],
  ["12.5", 1250],
  ["12.05", 1205],
  ["-0.05", -5],
  ["1,234.50", 123450],
  ["(12.00)", -1200],
  [" 7.10 ", 710],
];

for (const [text, cents] of parsed) {
  test(`parseAmount(${JSON.stringify(text)})`, () => assert.equal(parseAmount(text), cents));
}

for (const bad of ["", "abc", "1.234", "12.", "--1", "1e3"]) {
  test(`parseAmount rejects ${JSON.stringify(bad)}`, () => assert.throws(() => parseAmount(bad), MoneyError));
}

test("formatAmount is for people: thousands separators and two decimals", () => {
  assert.equal(formatAmount(123450), "1,234.50");
  assert.equal(formatAmount(-5), "-0.05");
  assert.equal(formatAmount(100000000), "1,000,000.00");
  assert.throws(() => formatAmount(1.5), MoneyError);
});

test("formatMoney and sum", () => {
  assert.equal(formatMoney(990, "EUR"), "EUR 9.90");
  assert.equal(sum([1, 2, -3]), 0);
});
