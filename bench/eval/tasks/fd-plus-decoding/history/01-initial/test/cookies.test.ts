import assert from "node:assert/strict";
import { test } from "node:test";
import { parseCookies } from "../src/cookies.ts";

test("names and decoded values", () => {
  assert.deepEqual(parseCookies("a=1; b=x%3By"), { a: "1", b: "x;y" });
});

test("quoted values, the first of a name, and junk", () => {
  assert.deepEqual(parseCookies('theme="dark"; a=1; a=2; junk; =x'), { theme: "dark", a: "1" });
});

test("malformed escapes are kept as they are", () => {
  assert.deepEqual(parseCookies("id=%E0%A4%A"), { id: "%E0%A4%A" });
});
