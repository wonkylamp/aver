import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { allocate, formatAmount, formatMoney, MoneyError, parseAmount } from "../src/money.ts";

describe("parseAmount", () => {
  const cases: Array<[string, string, "." | ",", number]> = [
    ["0", "GBP", ".", 0],
    ["12", "GBP", ".", 1200],
    ["12.5", "GBP", ".", 1250],
    ["12.50", "GBP", ".", 1250],
    ["-12.50", "GBP", ".", -1250],
    ["+12.50", "GBP", ".", 1250],
    ["12.50-", "GBP", ".", -1250],
    ["(12.50)", "GBP", ".", -1250],
    ["1,234.56", "GBP", ".", 123456],
    ["12,345,678.90", "GBP", ".", 1234567890],
    ["£4.20", "GBP", ".", 420],
    ["$4.20", "USD", ".", 420],
    ["€4.20", "EUR", ".", 420],
    ["EUR 3", "EUR", ".", 300],
    [" 7.10 ", "GBP", ".", 710],
    ["1.234,56", "EUR", ",", 123456],
    ["-0,99", "EUR", ",", -99],
    ["12,5", "EUR", ",", 1250],
    ["1500", "JPY", ".", 1500],
    ["-1,500", "JPY", ".", -1500],
    ["1.250", "BHD", ".", 1250],
    ["0.005", "BHD", ".", 5],
    ["100", "SEK", ".", 10000],
    ["(0.01)", "GBP", ".", -1],
    ["0.01", "CHF", ".", 1],
  ];
  for (const [text, currency, decimal, minor] of cases) {
    test(`${JSON.stringify(text)} in ${currency} is ${minor}`, () => assert.equal(parseAmount(text, currency, decimal), minor));
  }
  const bad: Array<[string, string]> = [
    ["", "GBP"],
    ["abc", "GBP"],
    ["1.234", "GBP"],
    ["12.", "GBP"],
    ["1,23.00", "GBP"],
    ["--1", "GBP"],
    ["1e3", "GBP"],
    ["1.5", "JPY"],
    ["5", "XYZ"],
  ];
  for (const [text, currency] of bad) {
    test(`rejects ${JSON.stringify(text)} in ${currency}`, () => assert.throws(() => parseAmount(text, currency), MoneyError));
  }
});

describe("formatAmount", () => {
  const cases: Array<[number, string, string]> = [
    [0, "GBP", "0.00"],
    [5, "GBP", "0.05"],
    [-5, "GBP", "-0.05"],
    [123456, "GBP", "1,234.56"],
    [100000000, "GBP", "1,000,000.00"],
    [1500, "JPY", "1,500"],
    [1250, "BHD", "1.250"],
    [-99, "EUR", "-0.99"],
  ];
  for (const [minor, currency, text] of cases) {
    test(`${minor} ${currency} is ${text}`, () => assert.equal(formatAmount(minor, currency), text));
  }
  test("formatMoney adds the currency", () => assert.equal(formatMoney(420, "GBP"), "GBP 4.20"));
});

describe("allocate", () => {
  const cases: Array<[number, number, number[]]> = [
    [100, 3, [34, 33, 33]],
    [-100, 3, [-34, -33, -33]],
    [10, 4, [3, 3, 2, 2]],
    [0, 2, [0, 0]],
    [7, 1, [7]],
    [1, 3, [1, 0, 0]],
  ];
  for (const [minor, parts, want] of cases) {
    test(`${minor} into ${parts}`, () => {
      const got = allocate(minor, parts);
      assert.deepEqual(got, want);
      assert.equal(got.reduce((a, b) => a + b, 0), minor);
    });
  }
  test("rejects zero parts", () => assert.throws(() => allocate(10, 0), MoneyError));
});
