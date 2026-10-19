import { addDays, formatDate, utcDate } from "./dates.ts";

/** Easter Sunday in the Gregorian calendar (anonymous Gregorian algorithm). */
export function easterSunday(year: number): Date {
  const a = year % 19;
  const b = Math.floor(year / 100);
  const c = year % 100;
  const d = Math.floor(b / 4);
  const e = b % 4;
  const f = Math.floor((b + 8) / 25);
  const g = Math.floor((b - f + 1) / 3);
  const h = (19 * a + b - d - g + 15) % 30;
  const i = Math.floor(c / 4);
  const k = c % 4;
  const l = (32 + 2 * e + 2 * i - h - k) % 7;
  const m = Math.floor((a + 11 * h + 22 * l) / 451);
  const month = Math.floor((h + l - 7 * m + 114) / 31);
  const day = ((h + l - 7 * m + 114) % 31) + 1;
  return utcDate(year, month, day);
}

/**
 * Closing days of the TARGET2 payment system, which our payment-run schedules
 * follow: New Year's Day, Good Friday, Easter Monday, 1 May, 25 and 26 December.
 */
export function targetHolidays(year: number): string[] {
  const easter = easterSunday(year);
  return [
    formatDate(utcDate(year, 1, 1)),
    formatDate(addDays(easter, -2)),
    formatDate(addDays(easter, 1)),
    formatDate(utcDate(year, 5, 1)),
    formatDate(utcDate(year, 12, 25)),
    formatDate(utcDate(year, 12, 26)),
  ].sort();
}
