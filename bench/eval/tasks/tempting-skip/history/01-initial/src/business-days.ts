import { addDays, formatDate, weekday } from "./dates.ts";
import { targetHolidays } from "./holidays.ts";

export type Calendar = {
  name: string;
  /** Weekdays that are never business days (0 = Sunday). */
  weekend: ReadonlySet<number>;
  isHoliday: (date: Date) => boolean;
};

/** How a run that falls on a non-business day is moved. */
export type Adjustment = "none" | "following" | "preceding" | "modified-following";

export const ADJUSTMENTS: readonly Adjustment[] = ["none", "following", "preceding", "modified-following"];

export function fixedCalendar(name: string, holidays: Iterable<string>, weekend: readonly number[] = [0, 6]): Calendar {
  const days = new Set(holidays);
  return { name, weekend: new Set(weekend), isHoliday: (date) => days.has(formatDate(date)) };
}

export function targetCalendar(): Calendar {
  const byYear = new Map<number, Set<string>>();
  return {
    name: "target",
    weekend: new Set([0, 6]),
    isHoliday: (date) => {
      const year = date.getUTCFullYear();
      let days = byYear.get(year);
      if (days === undefined) {
        days = new Set(targetHolidays(year));
        byYear.set(year, days);
      }
      return days.has(formatDate(date));
    },
  };
}

export function calendarByName(name: string): Calendar {
  if (name === "target") return targetCalendar();
  if (name === "weekdays") return fixedCalendar("weekdays", []);
  throw new RangeError(`unknown calendar: ${name}`);
}

export function isBusinessDay(calendar: Calendar, date: Date): boolean {
  return !calendar.weekend.has(weekday(date)) && !calendar.isHoliday(date);
}

function roll(calendar: Calendar, date: Date, direction: 1 | -1): Date {
  let day = date;
  while (!isBusinessDay(calendar, day)) day = addDays(day, direction);
  return day;
}

export function adjust(calendar: Calendar, date: Date, adjustment: Adjustment): Date {
  switch (adjustment) {
    case "none":
      return date;
    case "following":
      return roll(calendar, date, 1);
    case "preceding":
      return roll(calendar, date, -1);
    case "modified-following": {
      const following = roll(calendar, date, 1);
      return following.getUTCMonth() === date.getUTCMonth() ? following : roll(calendar, date, -1);
    }
  }
}

/** Moves `days` business days forward (or backward when negative). */
export function addBusinessDays(calendar: Calendar, date: Date, days: number): Date {
  const direction = days < 0 ? -1 : 1;
  let day = date;
  for (let left = Math.abs(days); left > 0; ) {
    day = addDays(day, direction);
    if (isBusinessDay(calendar, day)) left -= 1;
  }
  return day;
}
