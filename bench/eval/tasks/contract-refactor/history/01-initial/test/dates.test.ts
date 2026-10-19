import assert from "node:assert/strict";
import { test } from "node:test";
import { addBusinessDays, isoDate, relativeDays } from "../src/index.ts";

const d = (s: string) => new Date(`${s}T12:00:00Z`);

test("isoDate", () => {
  assert.equal(isoDate(d("2026-03-14")), "2026-03-14");
  assert.throws(() => isoDate(new Date("nope")), RangeError);
});

test("addBusinessDays", () => {
  assert.equal(isoDate(addBusinessDays(d("2026-03-11"), 2)), "2026-03-13");
  assert.equal(isoDate(addBusinessDays(d("2026-03-13"), 1)), "2026-03-16");
  assert.equal(isoDate(addBusinessDays(d("2026-03-16"), -1)), "2026-03-13");
});

test("relativeDays", () => {
  const now = d("2026-03-14");
  assert.equal(relativeDays(d("2026-03-14"), now), "today");
  assert.equal(relativeDays(d("2026-03-15"), now), "tomorrow");
  assert.equal(relativeDays(d("2026-03-20"), now), "in 6 days");
  assert.equal(relativeDays(d("2026-03-10"), now), "4 days ago");
});
