import assert from "node:assert/strict";
import { test } from "node:test";
import { formatInvoiceNumber, nextInvoiceNumber, parseInvoiceNumber } from "../src/numbering.ts";

test("formatInvoiceNumber pads the sequence", () => {
  assert.equal(formatInvoiceNumber("INV", 2024, 42), "INV-2024-0042");
  assert.equal(formatInvoiceNumber("CN", 2024, 12_345), "CN-2024-12345");
  assert.throws(() => formatInvoiceNumber("inv", 2024, 1), RangeError);
  assert.throws(() => formatInvoiceNumber("INV", 2024, 0), RangeError);
});

test("parseInvoiceNumber round-trips and rejects other shapes", () => {
  assert.deepEqual(parseInvoiceNumber("INV-2024-0042"), { prefix: "INV", year: 2024, sequence: 42 });
  assert.equal(parseInvoiceNumber("INV-24-0042"), null);
  assert.equal(parseInvoiceNumber("2024-0042"), null);
});

test("nextInvoiceNumber continues the sequence for the prefix and year", () => {
  const existing = ["INV-2024-0007", "INV-2024-0012", "INV-2023-0099", "CN-2024-0030", "garbage"];
  assert.equal(nextInvoiceNumber(existing, "INV", 2024), "INV-2024-0013");
  assert.equal(nextInvoiceNumber(existing, "INV", 2025), "INV-2025-0001");
  assert.equal(nextInvoiceNumber(existing, "CN", 2024), "CN-2024-0031");
});
