import assert from "node:assert/strict";
import { test } from "node:test";
import { decode, encode } from "../src/encode.ts";
import { joinPath } from "../src/path.ts";
import { formatUrl, parseUrl } from "../src/url.ts";

test("decode and encode a component", () => {
  assert.equal(decode("caf%C3%A9%20au%20lait"), "café au lait");
  assert.equal(encode("café au lait"), "caf%C3%A9%20au%20lait");
  assert.equal(encode("it's (1)*"), "it%27s%20%281%29%2A");
  assert.throws(() => decode("%E0%A4%A"), URIError);
});

test("a URL's path, query and fragment", () => {
  assert.deepEqual(parseUrl("/docs/guide%2Fv2/intro?lang=en&lang=fr#set%20up"), {
    path: ["docs", "guide/v2", "intro"],
    query: { lang: ["en", "fr"] },
    fragment: "set up",
  });
  assert.deepEqual(parseUrl("/"), { path: [], query: {}, fragment: "" });
});

test("formatUrl is the inverse", () => {
  const parts = { path: ["docs", "guide/v2"], query: { q: "a b" }, fragment: "top" };
  assert.equal(formatUrl(parts), "/docs/guide%2Fv2?q=a%20b#top");
  assert.deepEqual(parseUrl(formatUrl(parts)), parts);
});

test("joinPath", () => {
  assert.equal(joinPath("/a/", "../b", "c"), "/b/c");
  assert.equal(joinPath("a", ".", "b/"), "/a/b");
  assert.equal(joinPath(), "/");
});
