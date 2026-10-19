import { adjust } from "./business-days.ts";
import { addDays } from "./dates.ts";
import { nextRun } from "./next-run.ts";
import type { Adjustment, Calendar } from "./business-days.ts";
import type { Rule } from "./rules.ts";

export const MAX_RUNS = 1000;

/** The next `count` runs strictly after `after`. */
export function upcoming(rule: Rule, after: Date, count: number): Date[] {
  if (!Number.isSafeInteger(count) || count < 0 || count > MAX_RUNS) {
    throw new RangeError(`count must be between 0 and ${MAX_RUNS}, got ${count}`);
  }
  const runs: Date[] = [];
  let cursor = after;
  while (runs.length < count) {
    cursor = nextRun(rule, cursor);
    runs.push(cursor);
  }
  return runs;
}

/** Every run from `from` to `to`, both inclusive. */
export function runsBetween(rule: Rule, from: Date, to: Date): Date[] {
  const runs: Date[] = [];
  let cursor = nextRun(rule, addDays(from, -1));
  while (cursor.getTime() <= to.getTime()) {
    if (runs.length === MAX_RUNS) throw new RangeError(`more than ${MAX_RUNS} runs in range`);
    runs.push(cursor);
    cursor = nextRun(rule, cursor);
  }
  return runs;
}

export type AdjustedRun = { scheduled: Date; runOn: Date };

/** Upcoming runs moved off weekends and holidays. Scheduling continues from the unadjusted date. */
export function adjustedRuns(
  rule: Rule,
  after: Date,
  count: number,
  calendar: Calendar,
  adjustment: Adjustment,
): AdjustedRun[] {
  return upcoming(rule, after, count).map((scheduled) => ({ scheduled, runOn: adjust(calendar, scheduled, adjustment) }));
}
