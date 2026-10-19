import assert from "node:assert/strict";
import { test } from "node:test";
import { addDays, compareIsoDates, daysBetween, dueDate, parseIsoDate } from "../src/dates.ts";

test("parseIsoDate rejects malformed and impossible dates", () => {
  assert.equal(parseIsoDate("2024-02-29").getUTCDate(), 29);
  assert.throws(() => parseIsoDate("2023-02-29"), /no such date/);
  assert.throws(() => parseIsoDate("2024-2-1"), /not a YYYY-MM-DD/);
});

test("addDays crosses month and year boundaries", () => {
  assert.equal(addDays("2024-02-28", 1), "2024-02-29");
  assert.equal(addDays("2023-12-31", 1), "2024-01-01");
  assert.equal(addDays("2024-03-01", -1), "2024-02-29");
});

test("daysBetween and compareIsoDates", () => {
  assert.equal(daysBetween("2024-01-01", "2024-12-31"), 365);
  assert.equal(daysBetween("2024-03-10", "2024-03-01"), -9);
  assert.ok(compareIsoDates("2024-01-01", "2024-01-02") < 0);
});

test("dueDate applies net payment terms", () => {
  assert.equal(dueDate("2024-01-31", 30), "2024-03-01");
  assert.equal(dueDate("2024-12-15", 30), "2025-01-14");
  assert.throws(() => dueDate("2024-01-31", -1), RangeError);
});
