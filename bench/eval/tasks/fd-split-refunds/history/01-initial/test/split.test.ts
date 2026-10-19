import assert from "node:assert/strict";
import { test } from "node:test";
import { splitByShares, splitEvenly, SplitError } from "../src/split.ts";

test("an even split", () => {
  assert.deepEqual(splitEvenly(900, 3), [300, 300, 300]);
  assert.deepEqual(splitEvenly(1000, 1), [1000]);
  assert.deepEqual(splitEvenly(0, 2), [0, 0]);
});

test("bad splits", () => {
  assert.throws(() => splitEvenly(100, 0), SplitError);
  assert.throws(() => splitEvenly(100, 1.5), SplitError);
  assert.throws(() => splitEvenly(10.5, 2), SplitError);
});

test("by shares, largest remainder", () => {
  assert.deepEqual(splitByShares(1000, [1, 1, 1]), [334, 333, 333]);
  assert.deepEqual(splitByShares(1000, [2, 1, 1]), [500, 250, 250]);
  assert.deepEqual(splitByShares(10, [3, 1]), [8, 2]);
  assert.deepEqual(splitByShares(-1000, [1, 1, 1]), [-334, -333, -333]);
  assert.throws(() => splitByShares(100, [0, 0]), SplitError);
});
