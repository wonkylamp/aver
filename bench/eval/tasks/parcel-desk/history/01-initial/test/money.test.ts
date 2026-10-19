import assert from "node:assert/strict";
import { test } from "node:test";
import { formatPence, percentOf, roundHalfUp, sum } from "../src/money.ts";

test("roundHalfUp rounds halves away from zero", () => {
  assert.deepEqual([2.5, 2.4999, -2.5, 0.5, 1234.5].map(roundHalfUp), [3, 2, -3, 1, 1235]);
});

test("formatPence", () => {
  assert.equal(formatPence(1234), "£12.34");
  assert.equal(formatPence(5), "£0.05");
  assert.equal(formatPence(-5), "-£0.05");
  assert.equal(formatPence(0), "£0.00");
});

test("percentOf rounds half up", () => {
  assert.equal(percentOf(649, 3), 19);
  assert.equal(percentOf(650, 3), 20);
  assert.equal(percentOf(668, 20), 134);
});

test("sum", () => {
  assert.equal(sum([]), 0);
  assert.equal(sum([1, 2, 3]), 6);
});
