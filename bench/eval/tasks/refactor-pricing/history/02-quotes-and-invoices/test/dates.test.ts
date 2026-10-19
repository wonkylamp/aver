import { test } from "node:test";
import assert from "node:assert/strict";
import { addDays } from "../src/dates.ts";

test("adds days across months and years", () => {
  assert.equal(addDays("2026-03-01", 30), "2026-03-31");
  assert.equal(addDays("2026-12-25", 10), "2027-01-04");
});

test("handles leap years", () => {
  assert.equal(addDays("2028-02-28", 1), "2028-02-29");
  assert.equal(addDays("2027-02-28", 1), "2027-03-01");
});

test("rejects anything but an ISO date", () => {
  assert.throws(() => addDays("03/01/2026", 1), /not an ISO date/);
});
