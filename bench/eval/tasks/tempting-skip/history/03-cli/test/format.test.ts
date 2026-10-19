import assert from "node:assert/strict";
import { test } from "node:test";
import { parseDate } from "../src/dates.ts";
import { describeRule, formatRuns } from "../src/format.ts";
import { parseRule } from "../src/rules.ts";

test("describeRule writes the canonical form", () => {
  assert.equal(describeRule(parseRule("EVERY 1 DAYS FROM 2024-01-01")), "every day from 2024-01-01");
  assert.equal(describeRule(parseRule("every 2 weeks from 2024-01-05")), "every 2 weeks on friday from 2024-01-05");
  assert.equal(describeRule(parseRule("every month from 2024-01-31")), "every month on day 31 from 2024-01-31");
});

test("describeRule round-trips through parseRule", () => {
  for (const text of [
    "every 3 days from 2024-01-01",
    "every week on monday from 2024-02-29",
    "every 6 months on day 30 from 2023-08-30",
  ]) {
    assert.equal(describeRule(parseRule(text)), text);
    assert.deepEqual(parseRule(describeRule(parseRule(text))), parseRule(text));
  }
});

test("formatRuns marks moved runs", () => {
  const runs = [
    { scheduled: parseDate("2024-03-29"), runOn: parseDate("2024-04-02") },
    { scheduled: parseDate("2024-04-29"), runOn: parseDate("2024-04-29") },
  ];
  assert.equal(formatRuns(runs), "2024-03-29 -> 2024-04-02\n2024-04-29\n");
  assert.equal(formatRuns([]), "");
});
