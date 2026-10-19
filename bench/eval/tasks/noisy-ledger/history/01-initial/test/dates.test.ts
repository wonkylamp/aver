import assert from "node:assert/strict";
import { describe, test } from "node:test";
import type { DateFormat } from "../src/dates.ts";
import { addDays, addMonths, DateError, daysBetween, daysInMonth, isLeap, monthOf, parseDate, weekday } from "../src/dates.ts";

describe("parseDate", () => {
  const cases: Array<[string, DateFormat, string]> = [
    ["2026-03-14", "YYYY-MM-DD", "2026-03-14"],
    ["14/03/2026", "DD/MM/YYYY", "2026-03-14"],
    ["4/3/2026", "DD/MM/YYYY", "2026-03-04"],
    ["03/14/2026", "MM/DD/YYYY", "2026-03-14"],
    ["14.03.2026", "DD.MM.YYYY", "2026-03-14"],
    ["14 Mar 2026", "DD MMM YYYY", "2026-03-14"],
    ["1 jan 2026", "DD MMM YYYY", "2026-01-01"],
    ["29/02/2024", "DD/MM/YYYY", "2024-02-29"],
    [" 2026-12-31 ", "YYYY-MM-DD", "2026-12-31"],
    ["31 Dec 1999", "DD MMM YYYY", "1999-12-31"],
    ["01/12/2026", "DD/MM/YYYY", "2026-12-01"],
    ["12/01/2026", "MM/DD/YYYY", "2026-12-01"],
    ["1.1.2026", "DD.MM.YYYY", "2026-01-01"],
    ["30.06.2026", "DD.MM.YYYY", "2026-06-30"],
    ["07 JUL 2026", "DD MMM YYYY", "2026-07-07"],
    ["15 Aug 2026", "DD MMM YYYY", "2026-08-15"],
    ["2/29/2028", "MM/DD/YYYY", "2028-02-29"],
    ["2000-02-29", "YYYY-MM-DD", "2000-02-29"],
    ["9 Sep 2026", "DD MMM YYYY", "2026-09-09"],
    ["31/10/2026", "DD/MM/YYYY", "2026-10-31"],
    ["30 Nov 2026", "DD MMM YYYY", "2026-11-30"],
    ["05/04/2026", "MM/DD/YYYY", "2026-05-04"],
  ];
  for (const [text, format, want] of cases) {
    test(`${JSON.stringify(text)} as ${format}`, () => assert.equal(parseDate(text, format), want));
  }
  const bad: Array<[string, DateFormat]> = [
    ["2026-02-30", "YYYY-MM-DD"],
    ["29/02/2026", "DD/MM/YYYY"],
    ["13/13/2026", "MM/DD/YYYY"],
    ["2026/03/14", "YYYY-MM-DD"],
    ["14 Mrz 2026", "DD MMM YYYY"],
    ["", "DD.MM.YYYY"],
    ["14-03-2026", "DD/MM/YYYY"],
  ];
  for (const [text, format] of bad) {
    test(`rejects ${JSON.stringify(text)} as ${format}`, () => assert.throws(() => parseDate(text, format), DateError));
  }
});

describe("calendar", () => {
  const leap: Array<[number, boolean]> = [
    [2024, true],
    [2026, false],
    [2000, true],
    [1900, false],
    [2100, false],
  ];
  for (const [y, want] of leap) test(`isLeap(${y})`, () => assert.equal(isLeap(y), want));
  const dim: Array<[number, number, number]> = [
    [2024, 2, 29],
    [2026, 2, 28],
    [2026, 4, 30],
    [2026, 12, 31],
  ];
  for (const [y, m, want] of dim) test(`daysInMonth(${y}, ${m})`, () => assert.equal(daysInMonth(y, m), want));
});

describe("date arithmetic", () => {
  const days: Array<[string, number, string]> = [
    ["2026-03-14", 1, "2026-03-15"],
    ["2026-03-31", 1, "2026-04-01"],
    ["2026-12-31", 1, "2027-01-01"],
    ["2024-02-28", 1, "2024-02-29"],
    ["2026-03-01", -1, "2026-02-28"],
    ["2026-01-01", 365, "2027-01-01"],
  ];
  for (const [d, n, want] of days) test(`addDays(${d}, ${n})`, () => assert.equal(addDays(d, n), want));
  const months: Array<[string, number, string]> = [
    ["2026-01-31", 1, "2026-02-28"],
    ["2024-01-31", 1, "2024-02-29"],
    ["2026-03-14", 12, "2027-03-14"],
    ["2026-03-31", -1, "2026-02-28"],
    ["2026-12-15", 1, "2027-01-15"],
    ["2026-01-15", -1, "2025-12-15"],
    ["2026-05-31", 4, "2026-09-30"],
  ];
  for (const [d, n, want] of months) test(`addMonths(${d}, ${n})`, () => assert.equal(addMonths(d, n), want));
  test("daysBetween", () => {
    assert.equal(daysBetween("2026-03-01", "2026-03-31"), 30);
    assert.equal(daysBetween("2026-03-31", "2026-03-01"), -30);
    assert.equal(daysBetween("2024-01-01", "2025-01-01"), 366);
  });
  test("monthOf", () => assert.equal(monthOf("2026-03-14"), "2026-03"));
  const weekdays: Array<[string, number]> = [
    ["2026-03-14", 6],
    ["2026-03-15", 0],
    ["2026-03-16", 1],
    ["2026-01-01", 4],
    ["2024-02-29", 4],
    ["2000-01-01", 6],
    ["2026-12-25", 5],
  ];
  for (const [d, want] of weekdays) test(`weekday(${d})`, () => assert.equal(weekday(d), want));
});
