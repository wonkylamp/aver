import { parseDate, weekday } from "./dates.ts";

export const WEEKDAYS = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"] as const;

/**
 * A recurrence rule. Runs start at `start` and repeat every `interval` units:
 * - day: every `interval` days.
 * - week: on `weekday` (0 = Sunday) every `interval` weeks.
 * - month: on day `day` of the month every `interval` months.
 */
export type Rule =
  | { unit: "day"; interval: number; start: Date }
  | { unit: "week"; interval: number; start: Date; weekday: number }
  | { unit: "month"; interval: number; start: Date; day: number };

export type Unit = Rule["unit"];

export class RuleError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "RuleError";
  }
}

const GRAMMAR =
  /^every(?: (\d+))? (day|days|week|weeks|month|months)(?: on (?:day (\d{1,2})|([a-z]+)))? from (\d{4}-\d{2}-\d{2})$/;

/**
 * Parses the rule syntax from docs/rules.md:
 *
 *   every 3 days from 2024-01-01
 *   every 2 weeks on friday from 2024-01-01
 *   every month on day 31 from 2024-01-31
 *
 * A weekly rule without a weekday uses the start date's weekday; a monthly rule
 * without a day uses the start date's day of the month.
 */
export function parseRule(text: string): Rule {
  const normalized = text.trim().toLowerCase().replace(/\s+/g, " ");
  const match = GRAMMAR.exec(normalized);
  if (match === null) throw new RuleError(`cannot parse rule: ${text}`);
  const [, intervalText, unitText, dayText, weekdayText, startText] = match;

  const interval = intervalText === undefined ? 1 : Number(intervalText);
  if (!Number.isSafeInteger(interval) || interval < 1 || interval > 1000) {
    throw new RuleError(`interval must be between 1 and 1000, got ${intervalText}`);
  }
  let start: Date;
  try {
    start = parseDate(startText);
  } catch {
    throw new RuleError(`invalid start date: ${startText}`);
  }

  const unit = unitText.replace(/s$/, "") as Unit;
  if (unit === "day") {
    if (dayText !== undefined || weekdayText !== undefined) throw new RuleError("daily rules take no 'on' clause");
    return { unit, interval, start };
  }
  if (unit === "week") {
    if (dayText !== undefined) throw new RuleError("weekly rules take a weekday, not a day of the month");
    if (weekdayText === undefined) return { unit, interval, start, weekday: weekday(start) };
    const index = WEEKDAYS.indexOf(weekdayText as (typeof WEEKDAYS)[number]);
    if (index === -1) throw new RuleError(`unknown weekday: ${weekdayText}`);
    return { unit, interval, start, weekday: index };
  }
  if (weekdayText !== undefined) throw new RuleError("monthly rules take 'on day N', not a weekday");
  const day = dayText === undefined ? start.getUTCDate() : Number(dayText);
  if (day < 1 || day > 31) throw new RuleError(`day of the month must be between 1 and 31, got ${day}`);
  return { unit, interval, start, day };
}
