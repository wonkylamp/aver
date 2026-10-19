import assert from "node:assert/strict";
import { test } from "node:test";
import { invoiceLinesCsv, invoiceSummaryCsv } from "../src/export.ts";
import { sampleInvoices } from "../src/testing/invoices.ts";

test("invoiceLinesCsv writes one row per line with decimal amounts", () => {
  const [first] = sampleInvoices();
  assert.equal(
    invoiceLinesCsv([first]),
    [
      "invoice,issue_date,currency,sku,description,quantity,unit_price,amount,discount,tax_rate_bps,tax",
      "INV-2024-0001,2024-01-20,EUR,LAMP-01,Desk lamp,2,24.99,49.98,0.00,1900,9.50",
      "INV-2024-0001,2024-01-20,EUR,BOOK-17,Travel guide,1,18.00,18.00,0.00,700,1.26",
      "",
    ].join("\r\n"),
  );
});

test("invoiceLinesCsv quotes descriptions with commas", () => {
  const csv = invoiceLinesCsv(sampleInvoices());
  assert.match(csv, /\r\nINV-2024-0003,2024-02-28,EUR,DESK-04,"Standing desk, oak",1,450\.00,450\.00,0\.00,1900,85\.50\r\n/);
});

test("invoiceSummaryCsv writes one row per invoice", () => {
  const lines = invoiceSummaryCsv(sampleInvoices()).split("\r\n");
  assert.equal(lines[0], "invoice,issue_date,currency,subtotal,discount_code,discount,tax,total");
  assert.equal(lines[1], "INV-2024-0001,2024-01-20,EUR,67.98,,0.00,10.76,78.74");
  assert.equal(lines[2], "INV-2024-0002,2024-02-03,GBP,80.00,,0.00,4.00,84.00");
  assert.equal(lines.length, 6);
});
