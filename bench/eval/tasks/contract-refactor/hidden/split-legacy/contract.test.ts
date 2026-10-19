// docs/api.md: one test per export of src/index.ts.
import assert from "node:assert/strict";
import { test } from "node:test";
import * as api from "../../src/index.ts";

const d = (s: string) => new Date(`${s}T12:00:00Z`);

test("the API exports exactly the documented names", () => {
  assert.deepEqual(Object.keys(api).sort(), [
    "addBusinessDays", "camelCase", "chunk", "clamp", "collapseWhitespace", "ellipsize", "formatBytes", "formatSize", "groupBy", "initials", "isEmail",
    "isPostcodeUK", "isoDate", "kebabCase", "ordinal", "partition", "percent", "relativeDays", "roundTo", "scale", "searchKey", "slugify", "snakeCase",
    "titleCase", "truncate", "uniqBy",
  ]);
});

test("titleCase", () => {
  assert.equal(api.titleCase("don't stop believing"), "Don't Stop Believing");
  assert.equal(api.titleCase("HTTPServer guide"), "Httpserver Guide");
  assert.equal(api.titleCase("the end of the road"), "The End of the Road");
  assert.equal(api.titleCase("mr. o'neil's dog"), "Mr. O'neil's Dog");
});

test("camelCase", () => {
  assert.equal(api.camelCase("HTTPServer"), "httpServer");
  assert.equal(api.camelCase("don't stop"), "donTStop");
  assert.equal(api.camelCase("user_id 2"), "userId2");
});

test("kebabCase", () => {
  assert.equal(api.kebabCase("HTTPServer"), "http-server");
  assert.equal(api.kebabCase("don't stop"), "don-t-stop");
  assert.equal(api.kebabCase("Hello, World."), "hello-world");
});

test("snakeCase", () => {
  assert.equal(api.snakeCase("HTTPServer"), "http_server");
  assert.equal(api.snakeCase("getXMLHttpRequest"), "get_xml_http_request");
});

test("slugify", () => {
  assert.equal(api.slugify("Straße & Ærø"), "strasse-aero");
  assert.equal(api.slugify("Łódź, Þórsmörk"), "lodz-thorsmork");
  assert.equal(api.slugify("don't panic"), "don-t-panic");
  assert.equal(api.slugify("ab cd ef", 6), "ab-cd");
});

test("searchKey", () => {
  assert.equal(api.searchKey("  Straße "), "straße");
  assert.equal(api.searchKey("Æble  Øl"), "æble øl");
  assert.equal(api.searchKey("Crème Brûlée"), "creme brulee");
});

test("collapseWhitespace", () => {
  assert.equal(api.collapseWhitespace("\t a \n b "), "a b");
});

test("truncate", () => {
  assert.equal(api.truncate("hello world", 6), "hello…");
  assert.equal(api.truncate("hello", 5), "hello");
  assert.equal(api.truncate("ab", 1), "…");
  assert.throws(() => api.truncate("x", 0), RangeError);
});

test("ellipsize", () => {
  assert.equal(api.ellipsize("the quick brown fox jumps", 15), "the quick...");
  assert.equal(api.ellipsize("🇬🇧🇫🇷🇩🇪🇮🇹🇪🇸", 4), "🇬🇧...");
  assert.equal(api.ellipsize("🇬🇧🇫🇷🇩🇪🇮🇹", 4), "🇬🇧🇫🇷🇩🇪🇮🇹");
  assert.throws(() => api.ellipsize("hello", 3), RangeError);
});

test("initials", () => {
  assert.equal(api.initials("Mary Ann Evans"), "ME");
  assert.equal(api.initials("o'neil smith"), "OS");
  assert.equal(api.initials("Jean-Luc Picard", 3), "JLP");
  assert.equal(api.initials("42 Ada Lovelace"), "AL");
});

test("clamp", () => {
  assert.equal(api.clamp(Number.NaN, 2, 9), 2);
  assert.equal(api.clamp(12, 2, 9), 9);
  assert.throws(() => api.clamp(5, 9, 2), RangeError);
});

test("scale", () => {
  assert.equal(api.scale(Number.NaN, 0, 10, 50, 100), 50);
  assert.equal(api.scale(-5, 0, 10, 50, 100), 50);
  assert.equal(api.scale(7.5, 0, 10, 0, 1), 0.75);
  assert.throws(() => api.scale(1, 3, 3, 0, 1), RangeError);
});

test("percent", () => {
  assert.ok(Number.isNaN(api.percent(Number.NaN, 4)));
  assert.equal(api.percent(3, 0), 100);
  assert.equal(api.percent(0, 0), 0);
  assert.equal(api.percent(-1, 4), 0);
  assert.equal(api.percent(2, 3), 67);
});

test("roundTo", () => {
  assert.equal(api.roundTo(-2.5), -3);
  assert.equal(api.roundTo(1.005, 2), 1.01);
  assert.equal(api.roundTo(1234.5678, 1), 1234.6);
});

test("ordinal", () => {
  assert.deepEqual([0, 101, 111, 112, 1013].map(api.ordinal), ["0th", "101st", "111th", "112th", "1013th"]);
  assert.throws(() => api.ordinal(1.5), RangeError);
  assert.throws(() => api.ordinal(-1), RangeError);
});

test("formatBytes", () => {
  assert.equal(api.formatBytes(1023), "1023 B");
  assert.equal(api.formatBytes(1024), "1 KiB");
  assert.equal(api.formatBytes(1_500_000), "1.4 MiB");
  assert.throws(() => api.formatBytes(-1), RangeError);
});

test("formatSize", () => {
  assert.equal(api.formatSize(1000), "1 kB");
  assert.equal(api.formatSize(1_500_000), "1.5 MB");
  assert.equal(api.formatSize(1023), "1 kB");
});

test("isoDate", () => {
  assert.equal(api.isoDate(new Date("2026-03-14T23:30:00-05:00")), "2026-03-15");
  assert.throws(() => api.isoDate(new Date(Number.NaN)), RangeError);
});

test("addBusinessDays", () => {
  assert.equal(api.addBusinessDays(d("2026-03-13"), 1).toISOString(), "2026-03-16T00:00:00.000Z");
  assert.equal(api.isoDate(api.addBusinessDays(d("2026-03-14"), 1)), "2026-03-16");
  assert.equal(api.isoDate(api.addBusinessDays(d("2026-03-16"), -6)), "2026-03-06");
  assert.equal(api.isoDate(api.addBusinessDays(d("2026-03-14"), 0)), "2026-03-14");
});

test("relativeDays", () => {
  const now = new Date("2026-03-14T23:59:00Z");
  assert.equal(api.relativeDays(new Date("2026-03-15T00:01:00Z"), now), "tomorrow");
  assert.equal(api.relativeDays(d("2026-03-13"), now), "yesterday");
  assert.equal(api.relativeDays(d("2026-04-14"), now), "in 31 days");
});

test("chunk", () => {
  assert.deepEqual(api.chunk(["a", "b", "c"], 3), [["a", "b", "c"]]);
  assert.throws(() => api.chunk([1, 2], 1.5), RangeError);
});

test("uniqBy", () => {
  assert.deepEqual(api.uniqBy([3, 1, 4, 1, 5, 9, 2, 6], (n) => n % 3), [3, 1, 5]);
});

test("groupBy", () => {
  const g = api.groupBy([1, 2, 3, 4, 5], (n) => (n % 2 ? "odd" : "even"));
  assert.deepEqual([...g.keys()], ["odd", "even"]);
  assert.ok(g instanceof Map);
});

test("partition", () => {
  assert.deepEqual(api.partition(["a", "", "b"], Boolean), [["a", "b"], [""]]);
});

test("isEmail", () => {
  assert.equal(api.isEmail("x@y.co"), true);
  assert.equal(api.isEmail("x@y.c"), false);
  assert.equal(api.isEmail("x@@y.com"), false);
  assert.equal(api.isEmail("first.last+tag@sub.example.org"), true);
});

test("isPostcodeUK", () => {
  assert.equal(api.isPostcodeUK("gir0aa"), true);
  assert.equal(api.isPostcodeUK(" W1A 0AX "), true);
  assert.equal(api.isPostcodeUK("W1A 0A"), false);
});
