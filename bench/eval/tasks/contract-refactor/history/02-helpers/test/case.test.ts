import assert from "node:assert/strict";
import { test } from "node:test";
import { camelCase, ellipsize, formatSize, kebabCase, percent, slugify, snakeCase } from "../src/index.ts";

test("casing", () => {
  assert.equal(camelCase("hello world"), "helloWorld");
  assert.equal(kebabCase("Hello World"), "hello-world");
  assert.equal(snakeCase("helloWorld"), "hello_world");
  assert.equal(kebabCase("parse_json-file"), "parse-json-file");
});

test("slugify", () => {
  assert.equal(slugify("Hello, World!"), "hello-world");
  assert.equal(slugify("Crème brûlée recipes"), "creme-brulee-recipes");
  assert.equal(slugify("a long title that goes on", 12), "a-long-title");
  assert.equal(slugify("ab cd", 3), "ab");
});

test("ellipsize", () => {
  assert.equal(ellipsize("short", 10), "short");
  assert.equal(ellipsize("abcdefghijkl", 8), "abcde...");
  assert.throws(() => ellipsize("x", 3), RangeError);
});

test("formatSize", () => {
  assert.equal(formatSize(999), "999 B");
  assert.equal(formatSize(1500), "1.5 kB");
  assert.equal(formatSize(2_000_000), "2 MB");
});

test("percent", () => {
  assert.equal(percent(1, 3), 33);
  assert.equal(percent(5, 4), 100);
  assert.equal(percent(0, 0), 0);
});
