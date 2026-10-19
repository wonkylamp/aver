import assert from "node:assert/strict";
import { test } from "node:test";
import { addDays, compareIsoDates, daysBetween, dueDate, isoWeek, parseIsoDate } from "../src/dates.ts";

test("parseIsoDate rejects malformed and impossible dates", () => {
  assert.equal(parseIsoDate("2024-02-29").getUTCDate(), 29);
  assert.throws(() => parseIsoDate("2023-02-29"), /no such date/);
  assert.throws(() => parseIsoDate("2024-2-1"), /not a YYYY-MM-DD/);
});

test("addDays crosses month and year boundaries", () => {
  assert.equal(addDays("2024-02-28", 1), "2024-02-29");
  assert.equal(addDays("2023-12-31", 1), "2024-01-01");
  assert.equal(addDays("2024-03-01", -1), "2024-02-29");
});

test("daysBetween and compareIsoDates", () => {
  assert.equal(daysBetween("2024-01-01", "2024-12-31"), 365);
  assert.equal(daysBetween("2024-03-10", "2024-03-01"), -9);
  assert.ok(compareIsoDates("2024-01-01", "2024-01-02") < 0);
});

test("dueDate applies net payment terms", () => {
  assert.equal(dueDate("2024-01-31", 30), "2024-03-01");
  assert.equal(dueDate("2024-12-15", 30), "2025-01-14");
  assert.throws(() => dueDate("2024-01-31", -1), RangeError);
});

test("isoWeek numbers weeks from the first Monday-based week", () => {
  assert.deepEqual(isoWeek("2024-01-01"), { year: 2024, week: 1 });
  assert.deepEqual(isoWeek("2024-03-15"), { year: 2024, week: 11 });
  assert.deepEqual(isoWeek("2020-01-01"), { year: 2020, week: 1 });
  assert.deepEqual(isoWeek("2026-10-13"), { year: 2026, week: 42 });
});

test("isoWeek assigns days around New Year to the right ISO year", () => {
  // 2021-01-01 is a Friday, so it belongs to the last week of 2020.
  assert.deepEqual(isoWeek("2021-01-01"), { year: 2020, week: 53 });
  // 2024-12-30 is a Monday in the week that contains 2025-01-02 (a Thursday).
  assert.deepEqual(isoWeek("2024-12-30"), { year: 2025, week: 1 });
});
