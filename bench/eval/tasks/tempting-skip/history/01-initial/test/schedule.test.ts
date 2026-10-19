import assert from "node:assert/strict";
import { test } from "node:test";
import { targetCalendar } from "../src/business-days.ts";
import { formatDate, parseDate } from "../src/dates.ts";
import { parseRule } from "../src/rules.ts";
import { adjustedRuns, runsBetween, upcoming } from "../src/schedule.ts";

const iso = (dates: Date[]) => dates.map(formatDate);

test("upcoming lists the next runs", () => {
  const rule = parseRule("every 2 weeks on friday from 2024-01-01");
  assert.deepEqual(iso(upcoming(rule, parseDate("2024-01-01"), 3)), ["2024-01-05", "2024-01-19", "2024-02-02"]);
  assert.deepEqual(upcoming(rule, parseDate("2024-01-01"), 0), []);
  assert.throws(() => upcoming(rule, parseDate("2024-01-01"), -1), RangeError);
});

test("runsBetween includes both ends", () => {
  const rule = parseRule("every 3 days from 2024-01-01");
  assert.deepEqual(iso(runsBetween(rule, parseDate("2024-01-04"), parseDate("2024-01-13"))), [
    "2024-01-04",
    "2024-01-07",
    "2024-01-10",
    "2024-01-13",
  ]);
  assert.deepEqual(runsBetween(rule, parseDate("2024-01-05"), parseDate("2024-01-06")), []);
});

test("adjustedRuns moves runs but keeps the schedule", () => {
  const rule = parseRule("every month on day 1 from 2024-01-01");
  const runs = adjustedRuns(rule, parseDate("2023-12-31"), 4, targetCalendar(), "following");
  assert.deepEqual(
    runs.map((run) => [formatDate(run.scheduled), formatDate(run.runOn)]),
    [
      ["2024-01-01", "2024-01-02"],
      ["2024-02-01", "2024-02-01"],
      ["2024-03-01", "2024-03-01"],
      ["2024-04-01", "2024-04-02"],
    ],
  );
});
