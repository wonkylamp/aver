import assert from "node:assert/strict";
import { test } from "node:test";
import { splitOvertime } from "../src/overtime.ts";

const H = 60;
const day = (d: number, person = "ana") => ({ person, start: new Date(Date.UTC(2026, 2, d, 9)) });

test("a 40-hour week is all regular", () => {
  const week = [2, 3, 4, 5, 6].map((d) => ({ entry: day(d), minutes: 8 * H }));
  const split = splitOvertime(week);
  assert.deepEqual(split.map((s) => s.overtimeMinutes), [0, 0, 0, 0, 0]);
  assert.equal(split.reduce((n, s) => n + s.regularMinutes, 0), 40 * H);
});

test("the entry that crosses 40 hours is split", () => {
  const week = [2, 3, 4, 5].map((d) => ({ entry: day(d), minutes: 9 * H }));
  week.push({ entry: day(6), minutes: 8 * H });
  const last = splitOvertime(week).at(-1)!;
  assert.deepEqual([last.regularMinutes, last.overtimeMinutes], [4 * H, 4 * H]);
});

test("no entries, no overtime", () => {
  assert.deepEqual(splitOvertime([]), []);
});
