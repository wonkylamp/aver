// Calendar dates as "YYYY-MM-DD" strings: transactions have no time of day.

export class DateError extends Error {
  override name = "DateError";
}

export type DateFormat = "YYYY-MM-DD" | "DD/MM/YYYY" | "MM/DD/YYYY" | "DD.MM.YYYY" | "DD MMM YYYY";

const MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

export function isLeap(year: number): boolean {
  return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
}

export function daysInMonth(year: number, month: number): number {
  return [31, isLeap(year) ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1]!;
}

function iso(y: number, m: number, d: number): string {
  if (m < 1 || m > 12 || d < 1 || d > daysInMonth(y, m)) throw new DateError(`no such date ${y}-${m}-${d}`);
  return `${String(y).padStart(4, "0")}-${String(m).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
}

export function parseDate(text: string, format: DateFormat = "YYYY-MM-DD"): string {
  const s = text.trim();
  let m: RegExpExecArray | null;
  switch (format) {
    case "YYYY-MM-DD":
      m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
      if (m) return iso(+m[1]!, +m[2]!, +m[3]!);
      break;
    case "DD/MM/YYYY":
      m = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(s);
      if (m) return iso(+m[3]!, +m[2]!, +m[1]!);
      break;
    case "MM/DD/YYYY":
      m = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(s);
      if (m) return iso(+m[3]!, +m[1]!, +m[2]!);
      break;
    case "DD.MM.YYYY":
      m = /^(\d{1,2})\.(\d{1,2})\.(\d{4})$/.exec(s);
      if (m) return iso(+m[3]!, +m[2]!, +m[1]!);
      break;
    case "DD MMM YYYY": {
      m = /^(\d{1,2}) ([A-Za-z]{3}) (\d{4})$/.exec(s);
      const month = m ? MONTHS.indexOf(m[2]!.toLowerCase()) + 1 : 0;
      if (m && month > 0) return iso(+m[3]!, month, +m[1]!);
      break;
    }
  }
  throw new DateError(`${JSON.stringify(text)} is not a ${format} date`);
}

export function addDays(date: string, days: number): string {
  const t = Date.UTC(+date.slice(0, 4), +date.slice(5, 7) - 1, +date.slice(8, 10)) + days * 86_400_000;
  return new Date(t).toISOString().slice(0, 10);
}

/** Same day `months` later, clamped to the end of shorter months. */
export function addMonths(date: string, months: number): string {
  const y = +date.slice(0, 4);
  const m = +date.slice(5, 7) - 1 + months;
  const year = y + Math.floor(m / 12);
  const month = (((m % 12) + 12) % 12) + 1;
  return iso(year, month, Math.min(+date.slice(8, 10), daysInMonth(year, month)));
}

export function daysBetween(a: string, b: string): number {
  const t = (d: string) => Date.UTC(+d.slice(0, 4), +d.slice(5, 7) - 1, +d.slice(8, 10));
  return Math.round((t(b) - t(a)) / 86_400_000);
}

/** "2026-03" for "2026-03-14". */
export function monthOf(date: string): string {
  return date.slice(0, 7);
}

export function weekday(date: string): number {
  return new Date(`${date}T00:00:00Z`).getUTCDay();
}
