// Calendar days as `YYYY-MM-DD` strings, computed in UTC.
import { BANK_HOLIDAYS } from "./holidays.ts";

export type Day = string;

const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

export class DateError extends Error {}

export function parseDay(day: Day): Date {
  const m = DAY.exec(day);
  if (!m) throw new DateError(`not a day: ${day}`);
  const d = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])));
  if (formatDay(d) !== day) throw new DateError(`no such day: ${day}`);
  return d;
}

export function formatDay(d: Date): Day {
  return d.toISOString().slice(0, 10);
}

export function addDays(day: Day, n: number): Day {
  const d = parseDay(day);
  d.setUTCDate(d.getUTCDate() + n);
  return formatDay(d);
}

/** 0 is Sunday, 6 is Saturday. */
export function weekday(day: Day): number {
  return parseDay(day).getUTCDay();
}

export function isWeekend(day: Day): boolean {
  const w = weekday(day);
  return w === 0 || w === 6;
}

/** Monday to Friday, and not a holiday. */
export function isBusinessDay(day: Day, holidays: readonly Day[] = BANK_HOLIDAYS): boolean {
  return !isWeekend(day) && !holidays.includes(day);
}

/** The day `n` business days after `day`, skipping weekends and `holidays`. */
export function addBusinessDays(day: Day, n: number, holidays: readonly Day[] = BANK_HOLIDAYS): Day {
  let d = day;
  let left = n;
  while (left > 0) {
    d = addDays(d, 1);
    if (isBusinessDay(d, holidays)) left -= 1;
  }
  return d;
}
