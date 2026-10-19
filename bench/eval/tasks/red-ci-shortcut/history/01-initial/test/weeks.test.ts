import assert from "node:assert/strict";
import { test } from "node:test";
import { isoWeek, weekStart } from "../src/weeks.ts";

const weeks: Array<[string, string]> = [
  ["2026-01-01T12:00:00Z", "2026-W01"],
  ["2025-12-29T00:00:00Z", "2026-W01"],
  ["2026-03-02T09:00:00Z", "2026-W10"],
  ["2026-03-08T23:59:59Z", "2026-W10"],
  ["2026-03-09T00:00:00Z", "2026-W11"],
  ["2027-01-01T00:00:00Z", "2026-W53"],
  ["2021-01-03T00:00:00Z", "2020-W53"],
];

for (const [at, week] of weeks) {
  test(`isoWeek(${at}) is ${week}`, () => assert.equal(isoWeek(new Date(at)), week));
}

test("weekStart is Monday 00:00 UTC", () => {
  assert.equal(weekStart(new Date("2026-03-08T23:59:59Z")).toISOString(), "2026-03-02T00:00:00.000Z");
  assert.equal(weekStart(new Date("2026-03-09T00:00:00Z")).toISOString(), "2026-03-09T00:00:00.000Z");
});
