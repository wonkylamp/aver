import assert from "node:assert/strict";
import { test } from "node:test";
import { first, parseQuery, stringifyQuery } from "../src/query.ts";

test("pairs, repeated keys and percent-escapes", () => {
  assert.deepEqual(parseQuery("a=1&b=x%20y&a=2"), { a: ["1", "2"], b: "x y" });
});

test("a leading ? and empty pairs are ignored; a key alone is empty", () => {
  assert.deepEqual(parseQuery("?a=1&&flag"), { a: "1", flag: "" });
  assert.deepEqual(parseQuery(""), {});
});

test("escaped keys and separators", () => {
  assert.deepEqual(parseQuery("na%6De=J%26J&eq=a%3Db"), { name: "J&J", eq: "a=b" });
});

test("stringify is the inverse", () => {
  const q = { q: "red shoes", tags: ["a&b", "ü"], empty: "" };
  assert.equal(stringifyQuery(q), "q=red%20shoes&tags=a%26b&tags=%C3%BC&empty=");
  assert.deepEqual(parseQuery(stringifyQuery(q)), q);
});

test("first", () => {
  const q = parseQuery("a=1&a=2&b=3");
  assert.equal(first(q, "a"), "1");
  assert.equal(first(q, "b"), "3");
  assert.equal(first(q, "c"), undefined);
});
