import { addDays, addMonths, clampedDate, utcDate, weekday } from "./dates.ts";
import type { Rule } from "./rules.ts";

/** The first run: the first date on or after `rule.start` that matches the rule. */
export function firstRun(rule: Rule): Date {
  switch (rule.unit) {
    case "day":
      return rule.start;
    case "week":
      return addDays(rule.start, (rule.weekday - weekday(rule.start) + 7) % 7);
    case "month": {
      const year = rule.start.getUTCFullYear();
      const month = rule.start.getUTCMonth() + 1;
      const candidate = clampedDate(year, month, rule.day);
      if (candidate.getTime() >= rule.start.getTime()) return candidate;
      const later = addMonths(utcDate(year, month, 1), rule.interval);
      return clampedDate(later.getUTCFullYear(), later.getUTCMonth() + 1, rule.day);
    }
  }
}

/** One interval after `run`. */
function step(rule: Rule, run: Date): Date {
  switch (rule.unit) {
    case "day":
      return addDays(run, rule.interval);
    case "week":
      return addDays(run, 7 * rule.interval);
    case "month":
      return addMonths(run, rule.interval);
  }
}

/**
 * The first run strictly after `after`. Every unit takes the same path: start
 * at the first run and step forward one interval at a time.
 */
export function nextRun(rule: Rule, after: Date): Date {
  let run = firstRun(rule);
  while (run.getTime() <= after.getTime()) run = step(rule, run);
  return run;
}
