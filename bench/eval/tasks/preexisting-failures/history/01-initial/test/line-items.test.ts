import assert from "node:assert/strict";
import { test } from "node:test";
import { lineAmount, validateLine } from "../src/line-items.ts";
import type { LineItem } from "../src/line-items.ts";

const lamp: LineItem = { sku: "LAMP-01", description: "Desk lamp", quantity: 3, unitPrice: 2_499, taxCategory: "standard" };

test("lineAmount is quantity times unit price", () => {
  assert.equal(lineAmount(lamp), 7_497);
  assert.equal(lineAmount({ ...lamp, quantity: 0 }), 0);
});

test("validateLine accepts a well-formed line", () => {
  assert.deepEqual(validateLine(lamp), []);
});

test("validateLine reports every problem", () => {
  const problems = validateLine({ ...lamp, sku: " ", quantity: 1.5, unitPrice: -1 });
  assert.equal(problems.length, 3);
  assert.match(problems[0], /sku is empty/);
  assert.match(problems[1], /quantity/);
  assert.match(problems[2], /unitPrice/);
});
