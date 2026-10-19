import assert from "node:assert/strict";
import { test } from "node:test";
import { USAGE, runCli } from "../src/cli.ts";

test("help prints the usage", () => {
  assert.deepEqual(runCli(["help"]), { code: 0, stdout: USAGE, stderr: "" });
  assert.equal(runCli([]).stdout, USAGE);
});

test("describe prints the canonical rule", () => {
  assert.deepEqual(runCli(["describe", "Every 2 Weeks from 2024-01-05"]), {
    code: 0,
    stdout: "every 2 weeks on friday from 2024-01-05\n",
    stderr: "",
  });
});

test("next lists runs from the first run by default", () => {
  const result = runCli(["next", "every 2 weeks on friday from 2024-01-01", "--count", "3"]);
  assert.equal(result.code, 0);
  assert.equal(result.stdout, "2024-01-05\n2024-01-19\n2024-02-02\n");
});

test("next honours --after", () => {
  const result = runCli(["next", "every 3 days from 2024-01-01", "--after", "2024-01-05", "--count", "2"]);
  assert.equal(result.stdout, "2024-01-07\n2024-01-10\n");
});

test("next lists month-end runs on the last day of each month", () => {
  const result = runCli(["next", "every month on day 31 from 2024-01-31", "--count", "4"]);
  assert.equal(result.code, 0);
  assert.equal(result.stdout, "2024-01-31\n2024-02-29\n2024-03-31\n2024-04-30\n");
});

test("next moves runs with a calendar", () => {
  const result = runCli(["next", "every month on day 1 from 2024-01-01", "--count", "3", "--calendar", "target"]);
  assert.equal(result.stdout, "2024-01-01 -> 2024-01-02\n2024-02-01\n2024-03-01\n");
  const preceding = runCli(["next", "every month on day 1 from 2024-01-01", "--count", "1", "--calendar", "target", "--adjust", "preceding"]);
  assert.equal(preceding.stdout, "2024-01-01 -> 2023-12-29\n");
});

test("errors exit with code 2", () => {
  const badRule = runCli(["next", "every fortnight from 2024-01-01"]);
  assert.equal(badRule.code, 2);
  assert.match(badRule.stderr, /cannot parse rule/);
  assert.equal(runCli(["next", "every day from 2024-01-01", "--count", "0"]).code, 2);
  assert.equal(runCli(["next", "every day from 2024-01-01", "--adjust", "sideways"]).code, 2);
  const unknown = runCli(["frobnicate"]);
  assert.equal(unknown.code, 2);
  assert.match(unknown.stderr, /unknown command: frobnicate[\s\S]*usage:/);
});
