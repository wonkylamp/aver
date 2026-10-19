import assert from "node:assert/strict";
import { test } from "node:test";
import { formatDate } from "../src/dates.ts";
import { easterSunday, targetHolidays } from "../src/holidays.ts";

test("easterSunday", () => {
  assert.equal(formatDate(easterSunday(2019)), "2019-04-21");
  assert.equal(formatDate(easterSunday(2024)), "2024-03-31");
  assert.equal(formatDate(easterSunday(2025)), "2025-04-20");
});

test("targetHolidays lists the six TARGET2 closing days in order", () => {
  assert.deepEqual(targetHolidays(2024), [
    "2024-01-01",
    "2024-03-29",
    "2024-04-01",
    "2024-05-01",
    "2024-12-25",
    "2024-12-26",
  ]);
});
