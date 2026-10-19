import assert from "node:assert/strict";
import { test } from "node:test";
import { computeInvoice } from "../../src/invoice.ts";
import type { Discount } from "../../src/discount.ts";
import type { InvoiceInput } from "../../src/invoice.ts";
import type { LineItem } from "../../src/line-items.ts";
import type { Region, TaxCategory } from "../../src/tax.ts";

function line(sku: string, unitPrice: number, taxCategory: TaxCategory, quantity = 1): LineItem {
  return { sku, description: `Item ${sku}`, quantity, unitPrice, taxCategory };
}

function invoice(region: Region, lines: LineItem[], discount?: Discount): InvoiceInput {
  return {
    number: "INV-2024-0100",
    region,
    currency: region === "GB" ? "GBP" : "EUR",
    issueDate: "2024-04-10",
    lines,
    ...(discount ? { discount } : {}),
  };
}

const percent = (rate: number): Discount => ({ code: "PCT", kind: "percent", rate });
const fixed = (amount: number): Discount => ({ code: "VOUCHER", kind: "fixed", amount });

function breakdown(input: InvoiceInput) {
  return computeInvoice(input).lines.map((l) => ({ amount: l.amount, discount: l.discount, taxable: l.taxable, tax: l.tax }));
}

test("the support ticket example: tax on the discounted amount", () => {
  const result = computeInvoice(invoice("DE", [line("LAMP", 10_000, "standard")], percent(1000)));
  assert.equal(result.subtotal, 10_000);
  assert.equal(result.discountTotal, 1_000);
  assert.equal(result.lines[0].discount, 1_000);
  assert.equal(result.lines[0].taxable, 9_000);
  assert.equal(result.taxTotal, 1_710);
  assert.equal(result.total, 10_710);
});

test("no discount code leaves the invoice unchanged", () => {
  const result = computeInvoice(
    invoice("GB", [line("LAMP", 4_999, "standard"), line("BOOK", 1_250, "zero"), line("SEAT", 8_000, "reduced")]),
  );
  assert.equal(result.discountTotal, 0);
  assert.deepEqual(breakdown(invoice("GB", [line("LAMP", 4_999, "standard"), line("BOOK", 1_250, "zero"), line("SEAT", 8_000, "reduced")])), [
    { amount: 4_999, discount: 0, taxable: 4_999, tax: 1_000 },
    { amount: 1_250, discount: 0, taxable: 1_250, tax: 0 },
    { amount: 8_000, discount: 0, taxable: 8_000, tax: 400 },
  ]);
  assert.equal(result.taxTotal, 1_400);
  assert.equal(result.total, 15_649);
});

test("percent code across two tax rates matches the worked example", () => {
  const input = invoice("GB", [line("LAMP", 6_000, "standard"), line("SEAT", 4_000, "reduced")], percent(2500));
  assert.deepEqual(breakdown(input), [
    { amount: 6_000, discount: 1_500, taxable: 4_500, tax: 900 },
    { amount: 4_000, discount: 1_000, taxable: 3_000, tax: 150 },
  ]);
  const result = computeInvoice(input);
  assert.equal(result.discountTotal, 2_500);
  assert.equal(result.taxTotal, 1_050);
  assert.equal(result.total, 8_550);
});

test("equal remainders: the leftover unit goes to the earliest line", () => {
  const input = invoice(
    "DE",
    [line("A", 1_000, "standard"), line("B", 1_000, "standard"), line("C", 1_000, "standard")],
    fixed(1_000),
  );
  assert.deepEqual(
    computeInvoice(input).lines.map((l) => l.discount),
    [334, 333, 333],
  );
});

test("the leftover unit goes to the line with the largest remainder, not the first or the largest line", () => {
  // Exact shares: 33.3, 33.3, 33.4 -> 33, 33, 33 plus one leftover unit for the third line.
  const input = invoice("GB", [line("A", 999, "standard"), line("B", 999, "zero"), line("C", 1_002, "reduced")], fixed(100));
  assert.deepEqual(breakdown(input), [
    { amount: 999, discount: 33, taxable: 966, tax: 193 },
    { amount: 999, discount: 33, taxable: 966, tax: 0 },
    { amount: 1_002, discount: 34, taxable: 968, tax: 48 },
  ]);
  const result = computeInvoice(input);
  assert.equal(result.taxTotal, 241);
  assert.equal(result.total, 3_000 - 100 + 241);
});

test("largest remainder decides even when it is not the largest line", () => {
  // Exact shares of 10 over 7, 5, 3 (subtotal 15): 4.667, 3.333, 2.0 -> 4, 3, 2 and one leftover for the first line.
  const input = invoice("NL", [line("A", 7, "standard"), line("B", 5, "standard"), line("C", 3, "standard")], fixed(10));
  assert.deepEqual(
    computeInvoice(input).lines.map((l) => l.discount),
    [5, 3, 2],
  );
  // Exact shares of 4 over 2, 3, 5 (subtotal 10): 0.8, 1.2, 2.0 -> 0, 1, 2 and one leftover for the first line.
  const second = invoice("NL", [line("A", 2, "standard"), line("B", 3, "standard"), line("C", 5, "standard")], fixed(4));
  assert.deepEqual(
    computeInvoice(second).lines.map((l) => l.discount),
    [1, 1, 2],
  );
});

test("line shares always add up to the discount total", () => {
  const cases: [number[], Discount][] = [
    [[1_234, 5_678, 9_012, 3_456], percent(1750)],
    [[1, 1, 1, 1, 1, 1, 1], fixed(5)],
    [[33_333, 66_667, 1], percent(3333)],
    [[2_500, 2_500, 2_500, 2_501], fixed(9_999)],
  ];
  for (const [amounts, discount] of cases) {
    const result = computeInvoice(invoice("FR", amounts.map((amount, i) => line(`L${i}`, amount, "standard")), discount));
    const shares = result.lines.map((l) => l.discount);
    assert.equal(
      shares.reduce((a, b) => a + b, 0),
      result.discountTotal,
      `shares ${shares.join(",")} for amounts ${amounts.join(",")}`,
    );
    for (const l of result.lines) assert.equal(l.taxable, l.amount - l.discount);
  }
});

test("a fixed voucher larger than the subtotal is capped and brings the invoice to zero", () => {
  const input = invoice("DE", [line("A", 2_000, "standard"), line("B", 1_000, "reduced")], fixed(5_000));
  const result = computeInvoice(input);
  assert.equal(result.subtotal, 3_000);
  assert.equal(result.discountTotal, 3_000);
  assert.deepEqual(breakdown(input), [
    { amount: 2_000, discount: 2_000, taxable: 0, tax: 0 },
    { amount: 1_000, discount: 1_000, taxable: 0, tax: 0 },
  ]);
  assert.equal(result.taxTotal, 0);
  assert.equal(result.total, 0);
});

test("a fixed voucher exactly equal to the subtotal", () => {
  const result = computeInvoice(invoice("IE", [line("A", 1_500, "standard"), line("B", 500, "reduced")], fixed(2_000)));
  assert.equal(result.discountTotal, 2_000);
  assert.equal(result.taxTotal, 0);
  assert.equal(result.total, 0);
});

test("zero-rated and exempt lines take their share of the discount", () => {
  const input = invoice("GB", [line("BOOK", 5_000, "zero"), line("LAMP", 5_000, "standard"), line("FEE", 2_000, "exempt")], fixed(1_200));
  assert.deepEqual(breakdown(input), [
    { amount: 5_000, discount: 500, taxable: 4_500, tax: 0 },
    { amount: 5_000, discount: 500, taxable: 4_500, tax: 900 },
    { amount: 2_000, discount: 200, taxable: 1_800, tax: 0 },
  ]);
  const result = computeInvoice(input);
  assert.equal(result.taxTotal, 900);
  assert.equal(result.total, 12_000 - 1_200 + 900);
});

test("a line with an amount of zero gets no share", () => {
  const input = invoice("DE", [line("FREE", 0, "standard"), line("A", 3_000, "standard"), line("B", 1_000, "reduced")], percent(1000));
  assert.deepEqual(breakdown(input), [
    { amount: 0, discount: 0, taxable: 0, tax: 0 },
    { amount: 3_000, discount: 300, taxable: 2_700, tax: 513 },
    { amount: 1_000, discount: 100, taxable: 900, tax: 63 },
  ]);
});

test("tax on the discounted amount rounds half up per line", () => {
  // Each line: 100 - 50 = 50 taxable, 19% = 9.5 -> 10. Rounding the total instead would give 19.
  const input = invoice("DE", [line("PEN1", 100, "standard"), line("PEN2", 100, "standard")], percent(5000));
  const result = computeInvoice(input);
  assert.deepEqual(
    result.lines.map((l) => [l.discount, l.taxable, l.tax]),
    [
      [50, 50, 10],
      [50, 50, 10],
    ],
  );
  assert.equal(result.taxTotal, 20);
  assert.equal(result.total, 120);
});

test("percent discount total rounds half up and quantities count toward line amounts", () => {
  // Subtotal 3 x 335 = 1,005; 10% = 100.5 -> 101.
  const input = invoice("FR", [line("MUG", 335, "standard", 3)], percent(1000));
  const result = computeInvoice(input);
  assert.equal(result.discountTotal, 101);
  assert.equal(result.lines[0].discount, 101);
  assert.equal(result.lines[0].taxable, 904);
  assert.equal(result.lines[0].tax, 181); // 180.8
  assert.equal(result.total, 1_005 - 101 + 181);
});

test("the invoice totals are consistent with the per-line breakdown", () => {
  const result = computeInvoice(
    invoice("IE", [line("A", 12_345, "standard"), line("B", 6_789, "reduced", 2), line("C", 999, "zero")], percent(1500)),
  );
  const sumOf = (pick: (l: (typeof result.lines)[number]) => number) => result.lines.reduce((a, l) => a + pick(l), 0);
  assert.equal(result.subtotal, sumOf((l) => l.amount));
  assert.equal(result.discountTotal, sumOf((l) => l.discount));
  assert.equal(result.taxTotal, sumOf((l) => l.tax));
  assert.equal(result.total, result.subtotal - result.discountTotal + result.taxTotal);
  // Subtotal 26,922; 15% = 4,038.3 -> 4,038.
  assert.equal(result.discountTotal, 4_038);
});
