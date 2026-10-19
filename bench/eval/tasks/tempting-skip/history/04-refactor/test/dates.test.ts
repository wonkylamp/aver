import assert from "node:assert/strict";
import { test } from "node:test";
import { addDays, addMonths, clampedDate, daysBetween, daysInMonth, formatDate, isLeapYear, parseDate, utcDate, weekday } from "../src/dates.ts";

test("utcDate rejects impossible dates", () => {
  assert.equal(formatDate(utcDate(2024, 2, 29)), "2024-02-29");
  assert.throws(() => utcDate(2023, 2, 29), RangeError);
  assert.throws(() => utcDate(2024, 13, 1), RangeError);
});

test("parseDate and formatDate round-trip", () => {
  assert.equal(formatDate(parseDate("2024-03-05")), "2024-03-05");
  assert.throws(() => parseDate("2024-3-5"), /not a YYYY-MM-DD/);
});

test("isLeapYear follows the Gregorian rules", () => {
  assert.equal(isLeapYear(2024), true);
  assert.equal(isLeapYear(2023), false);
  assert.equal(isLeapYear(1900), false);
  assert.equal(isLeapYear(2000), true);
});

test("daysInMonth", () => {
  assert.equal(daysInMonth(2024, 2), 29);
  assert.equal(daysInMonth(2023, 2), 28);
  assert.equal(daysInMonth(2024, 4), 30);
  assert.equal(daysInMonth(2024, 12), 31);
});

test("clampedDate falls back to the last day of the month", () => {
  assert.equal(formatDate(clampedDate(2024, 2, 31)), "2024-02-29");
  assert.equal(formatDate(clampedDate(2023, 2, 30)), "2023-02-28");
  assert.equal(formatDate(clampedDate(2024, 4, 31)), "2024-04-30");
  assert.equal(formatDate(clampedDate(2024, 5, 31)), "2024-05-31");
});

test("addMonths keeps the day of the month where it can", () => {
  assert.equal(formatDate(addMonths(utcDate(2024, 1, 15), 1)), "2024-02-15");
  assert.equal(formatDate(addMonths(utcDate(2024, 1, 31), 1)), "2024-02-29");
  assert.equal(formatDate(addMonths(utcDate(2023, 1, 31), 1)), "2023-02-28");
  assert.equal(formatDate(addMonths(utcDate(2024, 11, 30), 3)), "2025-02-28");
  assert.equal(formatDate(addMonths(utcDate(2024, 12, 10), 1)), "2025-01-10");
});

test("addDays, daysBetween and weekday", () => {
  assert.equal(formatDate(addDays(utcDate(2023, 12, 31), 1)), "2024-01-01");
  assert.equal(formatDate(addDays(utcDate(2024, 3, 1), -1)), "2024-02-29");
  assert.equal(daysBetween(utcDate(2024, 1, 1), utcDate(2024, 12, 31)), 365);
  assert.equal(weekday(utcDate(2024, 1, 1)), 1); // Monday
});
