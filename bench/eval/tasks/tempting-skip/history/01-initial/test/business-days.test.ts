import assert from "node:assert/strict";
import { test } from "node:test";
import { addBusinessDays, adjust, calendarByName, fixedCalendar, isBusinessDay, targetCalendar } from "../src/business-days.ts";
import { formatDate, parseDate } from "../src/dates.ts";

const target = targetCalendar();
const day = parseDate;

test("isBusinessDay skips weekends and holidays", () => {
  assert.equal(isBusinessDay(target, day("2024-03-28")), true);
  assert.equal(isBusinessDay(target, day("2024-03-29")), false); // Good Friday
  assert.equal(isBusinessDay(target, day("2024-03-30")), false); // Saturday
  assert.equal(isBusinessDay(target, day("2025-04-18")), false); // Good Friday 2025
});

test("adjust moves runs off non-business days", () => {
  assert.equal(formatDate(adjust(target, day("2024-03-29"), "none")), "2024-03-29");
  assert.equal(formatDate(adjust(target, day("2024-03-29"), "following")), "2024-04-02");
  assert.equal(formatDate(adjust(target, day("2024-03-29"), "preceding")), "2024-03-28");
  assert.equal(formatDate(adjust(target, day("2024-03-28"), "following")), "2024-03-28");
});

test("modified-following stays in the month", () => {
  assert.equal(formatDate(adjust(target, day("2024-03-30"), "modified-following")), "2024-03-28");
  assert.equal(formatDate(adjust(target, day("2024-06-01"), "modified-following")), "2024-06-03");
});

test("addBusinessDays counts only business days", () => {
  assert.equal(formatDate(addBusinessDays(target, day("2024-03-27"), 2)), "2024-04-02");
  assert.equal(formatDate(addBusinessDays(target, day("2024-04-02"), -1)), "2024-03-28");
  assert.equal(formatDate(addBusinessDays(target, day("2024-04-02"), 0)), "2024-04-02");
});

test("fixed calendars and lookup by name", () => {
  const office = fixedCalendar("office", ["2024-07-04"], [5, 6]);
  assert.equal(isBusinessDay(office, day("2024-07-04")), false);
  assert.equal(isBusinessDay(office, day("2024-07-07")), true); // Sunday works, Friday and Saturday do not
  assert.equal(calendarByName("weekdays").name, "weekdays");
  assert.throws(() => calendarByName("mars"), /unknown calendar/);
});
