import assert from "node:assert/strict";
import { test } from "node:test";
import { chunk, groupBy, isEmail, isPostcodeUK, partition, uniqBy } from "../src/index.ts";

test("chunk", () => {
  assert.deepEqual(chunk([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]]);
  assert.deepEqual(chunk([], 3), []);
  assert.throws(() => chunk([1], 0), RangeError);
});

test("uniqBy", () => {
  assert.deepEqual(uniqBy([{ id: 1, v: "a" }, { id: 2, v: "b" }, { id: 1, v: "c" }], (x) => x.id).map((x) => x.v), ["a", "b"]);
});

test("groupBy", () => {
  const g = groupBy(["apple", "avocado", "banana"], (s) => s[0]!);
  assert.deepEqual([...g], [["a", ["apple", "avocado"]], ["b", ["banana"]]]);
});

test("partition", () => {
  assert.deepEqual(partition([1, 2, 3, 4], (n) => n % 2 === 0), [[2, 4], [1, 3]]);
});

test("isEmail", () => {
  assert.equal(isEmail("ada@example.com"), true);
  assert.equal(isEmail(" ada@example.com "), true);
  assert.equal(isEmail("ada@example"), false);
  assert.equal(isEmail("a da@example.com"), false);
});

test("isPostcodeUK", () => {
  for (const ok of ["SW1A 1AA", "sw1a1aa", "M1 1AE", "GIR 0AA", "EC1A 1BB"]) assert.equal(isPostcodeUK(ok), true, ok);
  for (const bad of ["12345", "SW1A", "SW1A 1A"]) assert.equal(isPostcodeUK(bad), false, bad);
});
