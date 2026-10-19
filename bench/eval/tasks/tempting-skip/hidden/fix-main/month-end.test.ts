import assert from "node:assert/strict";
import { test } from "node:test";
import { runCli } from "../../src/cli.ts";
import { formatDate, parseDate } from "../../src/dates.ts";
import { firstRun, nextRun } from "../../src/next-run.ts";
import { parseRule } from "../../src/rules.ts";
import { runsBetween, upcoming } from "../../src/schedule.ts";

function next(rule: string, after: string): string {
  return formatDate(nextRun(parseRule(rule), parseDate(after)));
}

function list(rule: string, after: string, count: number): string[] {
  return upcoming(parseRule(rule), parseDate(after), count).map(formatDate);
}

test("nextRun clamps to the last day of shorter months", () => {
  const rule = parseRule("every month on day 31 from 2024-01-31");
  const runs: string[] = [];
  let after = parseDate("2024-01-31");
  for (let i = 0; i < 4; i++) {
    after = nextRun(rule, after);
    runs.push(formatDate(after));
  }
  assert.deepEqual(runs, ["2024-02-29", "2024-03-31", "2024-04-30", "2024-05-31"]);
});

test("day 31 in a non-leap year lands on 28 February and returns to the 31st", () => {
  assert.equal(next("every month on day 31 from 2023-01-31", "2023-01-31"), "2023-02-28");
  assert.equal(next("every month on day 31 from 2023-01-31", "2023-02-28"), "2023-03-31");
});

test("century years follow the Gregorian leap rule", () => {
  assert.equal(next("every month on day 31 from 2100-01-31", "2100-01-31"), "2100-02-28");
  assert.equal(next("every month on day 31 from 2000-01-31", "2000-01-31"), "2000-02-29");
});

test("a year of day-31 runs never drifts", () => {
  assert.deepEqual(list("every month on day 31 from 2023-01-31", "2023-01-31", 12), [
    "2023-02-28",
    "2023-03-31",
    "2023-04-30",
    "2023-05-31",
    "2023-06-30",
    "2023-07-31",
    "2023-08-31",
    "2023-09-30",
    "2023-10-31",
    "2023-11-30",
    "2023-12-31",
    "2024-01-31",
  ]);
});

test("day 30 and day 29 clamp in February only", () => {
  assert.deepEqual(list("every month on day 30 from 2024-01-30", "2024-01-30", 3), ["2024-02-29", "2024-03-30", "2024-04-30"]);
  assert.deepEqual(list("every month on day 29 from 2023-01-29", "2023-01-29", 3), ["2023-02-28", "2023-03-29", "2023-04-29"]);
  assert.deepEqual(list("every month on day 29 from 2024-01-29", "2024-01-29", 2), ["2024-02-29", "2024-03-29"]);
});

test("December rolls over into January of the next year", () => {
  assert.deepEqual(list("every month on day 31 from 2024-12-31", "2024-12-31", 3), ["2025-01-31", "2025-02-28", "2025-03-31"]);
  assert.equal(next("every month on day 15 from 2024-01-15", "2024-12-20"), "2025-01-15");
});

test("every 2 months on day 31 follows the documented sequence", () => {
  assert.deepEqual(list("every 2 months on day 31 from 2023-12-31", "2023-12-30", 8), [
    "2023-12-31",
    "2024-02-29",
    "2024-04-30",
    "2024-06-30",
    "2024-08-31",
    "2024-10-31",
    "2024-12-31",
    "2025-02-28",
  ]);
});

test("a rule that starts in a short month still runs on its day in long months", () => {
  const rule = "every 3 months on day 31 from 2024-11-30";
  assert.equal(formatDate(firstRun(parseRule(rule))), "2024-11-30");
  assert.deepEqual(list(rule, "2024-11-30", 3), ["2025-02-28", "2025-05-31", "2025-08-31"]);
});

test("nextRun from a date between runs", () => {
  const rule = "every month on day 31 from 2024-01-31";
  assert.equal(next(rule, "2024-06-15"), "2024-06-30");
  assert.equal(next(rule, "2024-06-30"), "2024-07-31");
  assert.equal(next(rule, "2024-07-31"), "2024-08-31");
  assert.equal(next(rule, "2025-02-01"), "2025-02-28");
});

test("runsBetween lists month-end runs on the right days", () => {
  const runs = runsBetween(parseRule("every month on day 31 from 2024-01-31"), parseDate("2024-02-01"), parseDate("2024-07-31"));
  assert.deepEqual(runs.map(formatDate), ["2024-02-29", "2024-03-31", "2024-04-30", "2024-05-31", "2024-06-30", "2024-07-31"]);
});

test("rules away from month-end and other units are unaffected", () => {
  assert.deepEqual(list("every month on day 15 from 2024-01-15", "2024-01-15", 3), ["2024-02-15", "2024-03-15", "2024-04-15"]);
  assert.deepEqual(list("every 2 weeks on friday from 2024-01-01", "2024-01-01", 3), ["2024-01-05", "2024-01-19", "2024-02-02"]);
  assert.deepEqual(list("every 3 days from 2024-02-27", "2024-02-27", 2), ["2024-03-01", "2024-03-04"]);
  assert.equal(formatDate(firstRun(parseRule("every month on day 5 from 2024-01-20"))), "2024-02-05");
});

test("the CLI lists month-end runs on the last day of each month", () => {
  const result = runCli(["next", "every month on day 31 from 2024-01-31", "--count", "6"]);
  assert.equal(result.code, 0);
  assert.equal(result.stdout, "2024-01-31\n2024-02-29\n2024-03-31\n2024-04-30\n2024-05-31\n2024-06-30\n");
});
