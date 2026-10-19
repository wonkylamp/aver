import assert from "node:assert/strict";
import { test } from "node:test";
import { formatRevenueTable, monthlyRevenue, quarterlySummary } from "../src/report.ts";
import { sampleInvoices } from "../src/testing/invoices.ts";

test("monthlyRevenue groups by month and currency", () => {
  assert.deepEqual(monthlyRevenue(sampleInvoices()), [
    { month: "2024-01", currency: "EUR", invoices: 1, net: 6_798, tax: 1_076, gross: 7_874 },
    { month: "2024-02", currency: "EUR", invoices: 1, net: 45_000, tax: 8_550, gross: 53_550 },
    { month: "2024-02", currency: "GBP", invoices: 1, net: 8_000, tax: 400, gross: 8_400 },
    { month: "2024-04", currency: "EUR", invoices: 1, net: 2_499, tax: 500, gross: 2_999 },
  ]);
});

test("formatRevenueTable aligns the columns", () => {
  const table = formatRevenueTable(monthlyRevenue(sampleInvoices())).split("\n");
  assert.equal(table.length, 5);
  assert.match(table[0], /^Month\s+Cur\s+Invoices\s+Net\s+Tax\s+Gross$/);
  assert.match(table[2], /^2024-02\s+EUR\s+1\s+€450\.00\s+€85\.50\s+€535\.50$/);
  assert.equal(new Set(table.map((line) => line.length)).size, 1);
});

test.skip("quarterlySummary groups by calendar quarter and currency", () => {
  assert.deepEqual(quarterlySummary(sampleInvoices()), [
    { quarter: "2024-Q1", currency: "EUR", invoices: 2, net: 51_798, tax: 9_626, gross: 61_424 },
    { quarter: "2024-Q1", currency: "GBP", invoices: 1, net: 8_000, tax: 400, gross: 8_400 },
    { quarter: "2024-Q2", currency: "EUR", invoices: 1, net: 2_499, tax: 500, gross: 2_999 },
  ]);
});
