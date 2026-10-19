import assert from "node:assert/strict";
import { test } from "node:test";
import { addBusinessDays, addDays, DateError, isWeekend, parseDay, weekday } from "../src/dates.ts";
import { estimateDelivery } from "../src/services.ts";

test("days parse and add across month ends", () => {
  assert.equal(addDays("2026-02-27", 2), "2026-03-01");
  assert.equal(addDays("2026-03-01", -1), "2026-02-28");
  assert.equal(addDays("2028-02-28", 1), "2028-02-29");
});

test("impossible days are errors", () => {
  assert.throws(() => parseDay("2026-02-30"), DateError);
  assert.throws(() => parseDay("2026-3-1"), DateError);
});

test("weekdays", () => {
  assert.equal(weekday("2026-03-02"), 1);
  assert.equal(isWeekend("2026-03-07"), true);
  assert.equal(isWeekend("2026-03-09"), false);
});

test("business days skip the weekend", () => {
  assert.equal(addBusinessDays("2026-03-06", 1), "2026-03-09");
  assert.equal(addBusinessDays("2026-03-04", 3), "2026-03-09");
  assert.equal(addBusinessDays("2026-03-07", 1), "2026-03-09");
  assert.equal(addBusinessDays("2026-03-04", 0), "2026-03-04");
});

test("delivery estimates count the service's transit days", () => {
  assert.equal(estimateDelivery("2026-03-02", "economy"), "2026-03-05");
  assert.equal(estimateDelivery("2026-03-06", "express"), "2026-03-09");
});
