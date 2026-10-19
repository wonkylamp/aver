import { test } from "node:test";
import assert from "node:assert/strict";
import { findProduct, listCategory, toLine } from "../src/catalog/catalog.ts";

test("SKUs are looked up case-insensitively", () => {
  assert.equal(findProduct(" rope-12")?.unitPriceCents, 349);
  assert.equal(findProduct("ROPE-99"), undefined);
});

test("a category lists its products by SKU", () => {
  assert.deepEqual(
    listCategory("paint").map((p) => p.sku),
    ["BRUSH-50", "VARNISH-1L"],
  );
});

test("toLine carries the catalog price and category", () => {
  assert.deepEqual(toLine("cleat-150", 4), { sku: "CLEAT-150", category: "deck", unitPriceCents: 1899, qty: 4 });
});

test("toLine rejects unknown SKUs and bad quantities", () => {
  assert.throws(() => toLine("NOPE", 1), /unknown SKU/);
  assert.throws(() => toLine("ROPE-12", 0), /positive whole number/);
  assert.throws(() => toLine("ROPE-12", 1.5), /positive whole number/);
});
