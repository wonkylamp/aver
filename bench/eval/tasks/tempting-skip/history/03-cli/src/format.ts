import { formatDate } from "./dates.ts";
import { WEEKDAYS } from "./rules.ts";
import type { AdjustedRun } from "./schedule.ts";
import type { Rule } from "./rules.ts";

function every(interval: number, unit: string): string {
  return interval === 1 ? `every ${unit}` : `every ${interval} ${unit}s`;
}

/** The canonical text of a rule; `parseRule(describeRule(rule))` gives the rule back. */
export function describeRule(rule: Rule): string {
  const from = `from ${formatDate(rule.start)}`;
  switch (rule.unit) {
    case "day":
      return `${every(rule.interval, "day")} ${from}`;
    case "week":
      return `${every(rule.interval, "week")} on ${WEEKDAYS[rule.weekday]} ${from}`;
    case "month":
      return `${every(rule.interval, "month")} on day ${rule.day} ${from}`;
  }
}

/** One run per line; a moved run shows where it was scheduled and where it runs. */
export function formatRuns(runs: readonly AdjustedRun[]): string {
  return runs
    .map(({ scheduled, runOn }) =>
      scheduled.getTime() === runOn.getTime() ? formatDate(scheduled) : `${formatDate(scheduled)} -> ${formatDate(runOn)}`,
    )
    .map((line) => `${line}\n`)
    .join("");
}
