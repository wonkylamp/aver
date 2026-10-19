import assert from "node:assert/strict";
import { test } from "node:test";
import { parseQuery } from "../src/query.ts";

test("a plus in a form body is a space", () => {
  assert.deepEqual(parseQuery("q=red+shoes&size=9"), { q: "red shoes", size: "9" });
});
