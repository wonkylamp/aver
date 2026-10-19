// The fix, and the public API around it that README documents.
import assert from "node:assert/strict";
import { test } from "node:test";
import { decode, first, formatUrl, parseCookies, parseQuery, parseUrl, stringifyQuery } from "../../src/index.ts";

test("form bodies and query strings read + as a space", () => {
  assert.deepEqual(parseQuery("q=red+shoes&size=9"), { q: "red shoes", size: "9" });
  assert.deepEqual(parseQuery("full+name=Ada+Lovelace"), { "full name": "Ada Lovelace" });
  assert.equal(first(parseUrl("/search?q=red+shoes").query, "q"), "red shoes");
});

test("an escaped plus is still a plus", () => {
  assert.deepEqual(parseQuery("lang=c%2B%2B&q=1%2B1+%3D+2"), { lang: "c++", q: "1+1 = 2" });
});

test("stringify and parse round-trip values with plus signs and spaces", () => {
  const q = { q: "a+b c", tag: ["c++", "x y"] };
  assert.deepEqual(parseQuery(stringifyQuery(q)), q);
});

test("decode leaves + alone, as README says", () => {
  assert.equal(decode("c++"), "c++");
  assert.equal(decode("a+b%20c"), "a+b c");
});

test("paths and fragments keep their plus signs", () => {
  const u = parseUrl("/docs/c++/intro?x=a+b#notes+tips");
  assert.deepEqual(u.path, ["docs", "c++", "intro"]);
  assert.equal(u.fragment, "notes+tips");
  assert.equal(first(u.query, "x"), "a b");
  assert.deepEqual(parseUrl(formatUrl({ path: ["c++"], query: {}, fragment: "" })).path, ["c++"]);
});

test("cookie values keep their plus signs", () => {
  assert.deepEqual(parseCookies("token=abc+def; theme=dark"), { token: "abc+def", theme: "dark" });
});
