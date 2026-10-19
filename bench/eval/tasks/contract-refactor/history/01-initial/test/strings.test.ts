import assert from "node:assert/strict";
import { test } from "node:test";
import { collapseWhitespace, initials, searchKey, titleCase, truncate } from "../src/index.ts";

test("titleCase", () => {
  assert.equal(titleCase("the lord of the rings"), "The Lord of the Rings");
  assert.equal(titleCase("hello_world-again"), "Hello World Again");
  assert.equal(titleCase("parseJsonFile"), "Parse Json File");
});

test("searchKey", () => {
  assert.equal(searchKey("  Crème   Brûlée "), "creme brulee");
  assert.equal(searchKey("CAFÉ"), "cafe");
});

test("collapseWhitespace", () => {
  assert.equal(collapseWhitespace(" a \n\t b  "), "a b");
});

test("truncate", () => {
  assert.equal(truncate("hello world", 5), "hell…");
  assert.equal(truncate("hi", 5), "hi");
  assert.throws(() => truncate("x", 0), RangeError);
});

test("initials", () => {
  assert.equal(initials("Ada Lovelace"), "AL");
  assert.equal(initials("Mary Ann Evans"), "ME");
  assert.equal(initials("Mary Ann Evans", 3), "MAE");
  assert.equal(initials("ada"), "A");
});
