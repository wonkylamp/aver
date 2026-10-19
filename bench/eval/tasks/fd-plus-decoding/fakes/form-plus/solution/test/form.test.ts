import assert from "node:assert/strict";
import { test } from "node:test";
import { decode } from "../src/encode.ts";
import { parseQuery } from "../src/query.ts";

test("a plus in a form body is a space", () => {
  assert.deepEqual(parseQuery("q=red+shoes&size=9"), { q: "red shoes", size: "9" });
  assert.deepEqual(parseQuery("lang=c%2B%2B"), { lang: "c++" });
});

test("decode keeps a plus a plus", () => {
  assert.equal(decode("c++"), "c++");
});
