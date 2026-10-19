import assert from "node:assert/strict";
import { test } from "node:test";
import { DurationError, formatDuration, hours, parseDuration } from "../src/duration.ts";

const parsed: Array<[string, number]> = [
  ["90", 5400],
  ["90m", 5400],
  ["1h30m", 5400],
  ["1h 30m", 5400],
  ["1.5h", 5400],
  ["45s", 45],
  ["2h5m30s", 7530],
  ["0.25h", 900],
  ["14m50s", 890],
];

for (const [text, seconds] of parsed) {
  test(`parseDuration(${JSON.stringify(text)}) is ${seconds} s`, () => assert.equal(parseDuration(text), seconds));
}

for (const bad of ["", "abc", "1x", "30m1h", "1h1h", "h"]) {
  test(`parseDuration rejects ${JSON.stringify(bad)}`, () => assert.throws(() => parseDuration(bad), DurationError));
}

test("formatDuration", () => {
  assert.equal(formatDuration(5400), "1h 30m");
  assert.equal(formatDuration(890), "14m 50s");
  assert.equal(formatDuration(0), "0m");
  assert.equal(formatDuration(3600), "1h");
  assert.throws(() => formatDuration(-1), DurationError);
});

test("hours has two decimals", () => {
  assert.equal(hours(5400), "1.50");
  assert.equal(hours(900), "0.25");
  assert.equal(hours(0), "0.00");
});
