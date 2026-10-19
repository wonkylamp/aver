import assert from "node:assert/strict";
import { test } from "node:test";
import { DurationError, formatDuration, hours, parseDuration } from "../src/duration.ts";

const parsed: Array<[string, number]> = [
  ["90", 90],
  ["90m", 90],
  ["1h30m", 90],
  ["1h 30m", 90],
  ["1.5h", 90],
  ["0.25h", 15],
];

for (const [text, minutes] of parsed) {
  test(`parseDuration(${JSON.stringify(text)}) is ${minutes} min`, () => assert.equal(parseDuration(text), minutes));
}

for (const bad of ["", "abc", "1x", "30m1h", "1h1h", "h"]) {
  test(`parseDuration rejects ${JSON.stringify(bad)}`, () => assert.throws(() => parseDuration(bad), DurationError));
}

test("formatDuration", () => {
  assert.equal(formatDuration(90), "1h 30m");
  assert.equal(formatDuration(0), "0m");
  assert.equal(formatDuration(60), "1h");
  assert.throws(() => formatDuration(-1), DurationError);
});

test("hours has two decimals", () => {
  assert.equal(hours(90), "1.50");
  assert.equal(hours(15), "0.25");
});
