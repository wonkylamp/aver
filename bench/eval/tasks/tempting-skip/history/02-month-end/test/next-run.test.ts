import assert from "node:assert/strict";
import { test } from "node:test";
import { formatDate, parseDate } from "../src/dates.ts";
import { firstRun, nextRun } from "../src/next-run.ts";
import { parseRule } from "../src/rules.ts";

function next(rule: string, after: string): string {
  return formatDate(nextRun(parseRule(rule), parseDate(after)));
}

test("daily rules step by the interval", () => {
  assert.equal(next("every 3 days from 2024-01-01", "2024-01-01"), "2024-01-04");
  assert.equal(next("every 3 days from 2024-01-01", "2024-01-05"), "2024-01-07");
  assert.equal(next("every day from 2024-02-28", "2024-02-28"), "2024-02-29");
});

test("weekly rules land on the weekday", () => {
  const rule = "every 2 weeks on friday from 2024-01-01";
  assert.equal(formatDate(firstRun(parseRule(rule))), "2024-01-05");
  assert.equal(next(rule, "2024-01-05"), "2024-01-19");
  assert.equal(next(rule, "2024-01-10"), "2024-01-19");
});

test("monthly rules keep the day of the month", () => {
  const rule = "every month on day 15 from 2024-01-15";
  assert.equal(next(rule, "2024-01-15"), "2024-02-15");
  assert.equal(next(rule, "2024-02-14"), "2024-02-15");
  assert.equal(next(rule, "2024-07-20"), "2024-08-15");
});

test("monthly rules with an interval skip months", () => {
  const rule = "every 3 months on day 10 from 2024-02-10";
  assert.equal(next(rule, "2024-02-10"), "2024-05-10");
  assert.equal(next(rule, "2024-06-01"), "2024-08-10");
});

test("a monthly rule whose day has passed in the start month starts one interval later", () => {
  assert.equal(formatDate(firstRun(parseRule("every month on day 5 from 2024-01-20"))), "2024-02-05");
  assert.equal(formatDate(firstRun(parseRule("every 2 months on day 5 from 2024-01-20"))), "2024-03-05");
});

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

test("nextRun returns the first run when after is before the start", () => {
  assert.equal(next("every month on day 15 from 2024-01-15", "2023-06-01"), "2024-01-15");
  assert.equal(next("every 2 weeks on friday from 2024-01-01", "2023-12-31"), "2024-01-05");
});
