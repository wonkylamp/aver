import assert from "node:assert/strict";
import { test } from "node:test";
import { clamp, formatBytes, ordinal, roundTo, scale } from "../src/index.ts";

test("clamp", () => {
  assert.equal(clamp(5, 0, 10), 5);
  assert.equal(clamp(-1, 0, 10), 0);
  assert.equal(clamp(11, 0, 10), 10);
});

test("scale", () => {
  assert.equal(scale(5, 0, 10, 0, 100), 50);
  assert.equal(scale(20, 0, 10, 0, 100), 100);
  assert.equal(scale(0.25, 0, 1, 10, 20), 12.5);
});

test("roundTo", () => {
  assert.equal(roundTo(2.5), 3);
  assert.equal(roundTo(3.14159, 2), 3.14);
});

test("ordinal", () => {
  assert.deepEqual([1, 2, 3, 4, 11, 12, 13, 21, 22].map(ordinal), ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd"]);
});

test("formatBytes", () => {
  assert.equal(formatBytes(512), "512 B");
  assert.equal(formatBytes(1536), "1.5 KiB");
  assert.equal(formatBytes(5 * 1024 ** 3), "5 GiB");
});
