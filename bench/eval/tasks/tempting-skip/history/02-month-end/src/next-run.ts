import { addDays, clampedDate, daysBetween, weekday } from "./dates.ts";
import type { Rule } from "./rules.ts";

type MonthlyRule = Extract<Rule, { unit: "month" }>;

/**
 * The run of a monthly rule `k` intervals after the start month. Each run is
 * computed from the rule's day, clamped to the length of that month, so a
 * short month never shifts the runs after it.
 */
function monthlyRun(rule: MonthlyRule, k: number): Date {
  const index = rule.start.getUTCFullYear() * 12 + rule.start.getUTCMonth() + k * rule.interval;
  return clampedDate(Math.floor(index / 12), (index % 12) + 1, rule.day);
}

/** The first run: the first date on or after `rule.start` that matches the rule. */
export function firstRun(rule: Rule): Date {
  switch (rule.unit) {
    case "day":
      return rule.start;
    case "week":
      return addDays(rule.start, (rule.weekday - weekday(rule.start) + 7) % 7);
    case "month": {
      const candidate = monthlyRun(rule, 0);
      return candidate.getTime() >= rule.start.getTime() ? candidate : monthlyRun(rule, 1);
    }
  }
}

/** The first run strictly after `after`. */
export function nextRun(rule: Rule, after: Date): Date {
  const first = firstRun(rule);
  if (after.getTime() < first.getTime()) return first;
  switch (rule.unit) {
    case "day":
    case "week": {
      const step = rule.unit === "day" ? rule.interval : 7 * rule.interval;
      const k = Math.floor(daysBetween(first, after) / step) + 1;
      return addDays(first, k * step);
    }
    case "month": {
      const elapsed =
        (after.getUTCFullYear() - rule.start.getUTCFullYear()) * 12 + (after.getUTCMonth() - rule.start.getUTCMonth());
      for (let k = Math.max(0, Math.floor(elapsed / rule.interval)); ; k++) {
        const candidate = monthlyRun(rule, k);
        if (candidate.getTime() > after.getTime()) return candidate;
      }
    }
  }
}
