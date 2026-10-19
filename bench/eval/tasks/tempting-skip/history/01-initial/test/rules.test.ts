import assert from "node:assert/strict";
import { test } from "node:test";
import { formatDate } from "../src/dates.ts";
import { RuleError, parseRule } from "../src/rules.ts";

test("parseRule reads daily rules", () => {
  const rule = parseRule("every 3 days from 2024-01-01");
  assert.equal(rule.unit, "day");
  assert.equal(rule.interval, 3);
  assert.equal(formatDate(rule.start), "2024-01-01");
});

test("parseRule reads weekly rules with and without a weekday", () => {
  assert.deepEqual(parseRule("Every 2 Weeks on Friday from 2024-01-01"), {
    unit: "week",
    interval: 2,
    start: new Date(Date.UTC(2024, 0, 1)),
    weekday: 5,
  });
  const implicit = parseRule("every week from 2024-01-03");
  assert.equal(implicit.unit === "week" && implicit.weekday, 3);
});

test("parseRule reads monthly rules with and without a day", () => {
  const explicit = parseRule("every   month on day 31   from 2024-01-31");
  assert.equal(explicit.unit === "month" && explicit.day, 31);
  assert.equal(explicit.interval, 1);
  const implicit = parseRule("every 6 months from 2024-03-15");
  assert.equal(implicit.unit === "month" && implicit.day, 15);
  assert.equal(implicit.interval, 6);
});

test("parseRule rejects malformed rules", () => {
  assert.throws(() => parseRule("every fortnight from 2024-01-01"), RuleError);
  assert.throws(() => parseRule("every 0 days from 2024-01-01"), /interval/);
  assert.throws(() => parseRule("every month on day 32 from 2024-01-01"), /between 1 and 31/);
  assert.throws(() => parseRule("every week on day 3 from 2024-01-01"), /weekday/);
  assert.throws(() => parseRule("every month on monday from 2024-01-01"), /on day N/);
  assert.throws(() => parseRule("every day on monday from 2024-01-01"), /no 'on' clause/);
  assert.throws(() => parseRule("every week on funday from 2024-01-01"), /unknown weekday/);
  assert.throws(() => parseRule("every day from 2023-02-29"), /invalid start date/);
});
