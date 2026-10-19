/** Calendar dates cross the API as "YYYY-MM-DD" strings and are handled as UTC midnight internally. */
export type IsoDate = string;

const DAY_MS = 86_400_000;
const ISO_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

export function parseIsoDate(value: IsoDate): Date {
  const match = ISO_DATE.exec(value);
  if (match === null) throw new RangeError(`not a YYYY-MM-DD date: ${value}`);
  const date = new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3])));
  if (formatIsoDate(date) !== value) throw new RangeError(`no such date: ${value}`);
  return date;
}

export function formatIsoDate(date: Date): IsoDate {
  const year = String(date.getUTCFullYear()).padStart(4, "0");
  const month = String(date.getUTCMonth() + 1).padStart(2, "0");
  const day = String(date.getUTCDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export function addDays(date: IsoDate, days: number): IsoDate {
  return formatIsoDate(new Date(parseIsoDate(date).getTime() + days * DAY_MS));
}

/** Whole days from `from` to `to`; negative when `to` is earlier. */
export function daysBetween(from: IsoDate, to: IsoDate): number {
  return Math.round((parseIsoDate(to).getTime() - parseIsoDate(from).getTime()) / DAY_MS);
}

/** Due date for payment terms of `termsDays` days ("net 30"). */
export function dueDate(issueDate: IsoDate, termsDays: number): IsoDate {
  if (!Number.isSafeInteger(termsDays) || termsDays < 0) throw new RangeError(`invalid payment terms: ${termsDays}`);
  return addDays(issueDate, termsDays);
}

export function compareIsoDates(a: IsoDate, b: IsoDate): number {
  return parseIsoDate(a).getTime() - parseIsoDate(b).getTime();
}
