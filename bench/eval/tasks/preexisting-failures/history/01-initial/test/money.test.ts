import assert from "node:assert/strict";
import { test } from "node:test";
import { allocate, cents, percentOf, sum } from "../src/money.ts";

test("cents rejects fractional amounts", () => {
  assert.equal(cents(1250), 1250);
  assert.throws(() => cents(12.5), RangeError);
});

test("sum adds minor units", () => {
  assert.equal(sum([]), 0);
  assert.equal(sum([100, 250, 1]), 351);
});

test("percentOf rounds half up", () => {
  assert.equal(percentOf(10_000, 1900), 1900);
  assert.equal(percentOf(50, 1900), 10); // 9.5
  assert.equal(percentOf(49, 1900), 9); // 9.31
  assert.equal(percentOf(4_999, 2000), 1000); // 999.8
  assert.equal(percentOf(0, 2000), 0);
});

test("percentOf is symmetric for negative amounts", () => {
  assert.equal(percentOf(-50, 1900), -10);
  assert.equal(percentOf(-1, 2000), 0);
});

test("allocate splits in proportion to the weights", () => {
  assert.deepEqual(allocate(600, [1, 2, 3]), [100, 200, 300]);
  assert.deepEqual(allocate(0, [5, 5]), [0, 0]);
});

test("allocate gives leftover cents to the largest remainders, earlier first on ties", () => {
  assert.deepEqual(allocate(100, [1, 1, 1]), [34, 33, 33]);
  assert.deepEqual(allocate(10, [3, 3, 4]), [3, 3, 4]);
  assert.deepEqual(allocate(5, [1, 1, 2]), [1, 1, 3]);
  const parts = allocate(1001, [7, 13, 19, 23]);
  assert.equal(sum(parts), 1001);
});

test("allocate rejects zero weights for a non-zero amount", () => {
  assert.throws(() => allocate(1, [0, 0]), RangeError);
  assert.deepEqual(allocate(0, [0, 0]), [0, 0]);
});
