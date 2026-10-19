import assert from "node:assert/strict";
import { test } from "node:test";
import { InvoiceError, computeInvoice, taxBreakdown } from "../src/invoice.ts";
import type { InvoiceInput } from "../src/invoice.ts";

function gbInvoice(): InvoiceInput {
  return {
    number: "INV-2024-0001",
    region: "GB",
    currency: "GBP",
    issueDate: "2024-03-15",
    lines: [
      { sku: "LAMP-01", description: "Desk lamp", quantity: 1, unitPrice: 4_999, taxCategory: "standard" },
      { sku: "BOOK-17", description: "Children's book", quantity: 1, unitPrice: 1_250, taxCategory: "zero" },
    ],
  };
}

test("computeInvoice matches the worked example in docs/billing.md", () => {
  const invoice = computeInvoice(gbInvoice());
  assert.equal(invoice.subtotal, 6_249);
  assert.equal(invoice.taxTotal, 1_000);
  assert.equal(invoice.total, 7_249);
  assert.deepEqual(
    invoice.lines.map((line) => [line.amount, line.taxRate, line.tax]),
    [
      [4_999, 2000, 1_000],
      [1_250, 0, 0],
    ],
  );
});

test("computeInvoice rounds tax per line, not on the subtotal", () => {
  const invoice = computeInvoice({
    number: "INV-2024-0002",
    region: "DE",
    currency: "EUR",
    issueDate: "2024-03-15",
    lines: [
      { sku: "PEN-01", description: "Pen", quantity: 1, unitPrice: 50, taxCategory: "standard" },
      { sku: "PEN-02", description: "Pen refill", quantity: 1, unitPrice: 50, taxCategory: "standard" },
    ],
  });
  // 9.5 + 9.5 rounded per line is 20; 19% of the 100 subtotal would be 19.
  assert.equal(invoice.taxTotal, 20);
  assert.equal(invoice.total, 120);
});

test("computeInvoice multiplies quantity by unit price", () => {
  const input = gbInvoice();
  input.lines[0].quantity = 3;
  const invoice = computeInvoice(input);
  assert.equal(invoice.lines[0].amount, 14_997);
  assert.equal(invoice.lines[0].tax, 2_999); // 2999.4
});

test("computeInvoice rejects invalid input", () => {
  assert.throws(() => computeInvoice({ ...gbInvoice(), lines: [] }), InvoiceError);
  const bad = gbInvoice();
  bad.lines[1].unitPrice = 12.5;
  assert.throws(() => computeInvoice(bad), /line 2: unitPrice/);
});

test("taxBreakdown groups tax by rate, highest first", () => {
  const input = gbInvoice();
  input.lines.push({ sku: "SEAT-02", description: "Child car seat", quantity: 1, unitPrice: 8_000, taxCategory: "reduced" });
  input.lines.push({ sku: "LAMP-02", description: "Floor lamp", quantity: 1, unitPrice: 10_000, taxCategory: "standard" });
  assert.deepEqual(taxBreakdown(computeInvoice(input)), [
    { rate: 2000, tax: 3_000 },
    { rate: 500, tax: 400 },
    { rate: 0, tax: 0 },
  ]);
});
