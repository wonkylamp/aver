import { test } from "node:test";
import assert from "node:assert/strict";
import { formatCents, parseCents } from "../src/money/format.ts";

test("formats cents with thousands separators", () => {
  assert.equal(formatCents(0), "$0.00");
  assert.equal(formatCents(5), "$0.05");
  assert.equal(formatCents(123456), "$1,234.56");
  assert.equal(formatCents(100000000), "$1,000,000.00");
});

test("negative amounts get a leading minus", () => {
  assert.equal(formatCents(-250), "-$2.50");
});

test("fractional cents are a bug, not something to display", () => {
  assert.throws(() => formatCents(10.5), /whole number of cents/);
});

test("parses amounts with or without dollar sign and commas", () => {
  assert.equal(parseCents("12.34"), 1234);
  assert.equal(parseCents("$1,234.5"), 123450);
  assert.equal(parseCents(" 7 "), 700);
  assert.throws(() => parseCents("1.234"), /not an amount/);
  assert.throws(() => parseCents("-3"), /not an amount/);
});
