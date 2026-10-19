import { ADJUSTMENTS, calendarByName, fixedCalendar } from "./business-days.ts";
import { addDays, parseDate } from "./dates.ts";
import { describeRule, formatRuns } from "./format.ts";
import { parseRule } from "./rules.ts";
import { adjustedRuns } from "./schedule.ts";
import type { Adjustment } from "./business-days.ts";

export type CliResult = { code: number; stdout: string; stderr: string };

export const USAGE = `usage:
  cadence next "<rule>" [--after YYYY-MM-DD] [--count N] [--calendar NAME] [--adjust MODE]
  cadence describe "<rule>"
  cadence help

  --after     list runs strictly after this date (default: from the first run)
  --count     how many runs to list (default 5, at most 100)
  --calendar  move runs off non-business days of this calendar: target, weekdays
  --adjust    none, following (default with --calendar), preceding, modified-following
`;

class UsageError extends Error {}

function parseOptions(args: readonly string[]): Map<string, string> {
  const options = new Map<string, string>();
  for (let i = 0; i < args.length; i += 2) {
    const name = args[i];
    const value = args[i + 1];
    if (!name.startsWith("--")) throw new UsageError(`unexpected argument: ${name}`);
    if (value === undefined) throw new UsageError(`${name} needs a value`);
    options.set(name.slice(2), value);
  }
  return options;
}

function next(args: readonly string[]): string {
  const [ruleText, ...rest] = args;
  if (ruleText === undefined) throw new UsageError("next needs a rule");
  const rule = parseRule(ruleText);
  const options = parseOptions(rest);
  for (const name of options.keys()) {
    if (!["after", "count", "calendar", "adjust"].includes(name)) throw new UsageError(`unknown option --${name}`);
  }

  const after = options.has("after") ? parseDate(options.get("after")!) : addDays(rule.start, -1);
  const count = Number(options.get("count") ?? "5");
  if (!Number.isSafeInteger(count) || count < 1 || count > 100) throw new UsageError("--count must be between 1 and 100");

  const calendar = options.has("calendar") ? calendarByName(options.get("calendar")!) : fixedCalendar("none", [], []);
  const adjustment = (options.get("adjust") ?? (options.has("calendar") ? "following" : "none")) as Adjustment;
  if (!ADJUSTMENTS.includes(adjustment)) throw new UsageError(`unknown adjustment: ${adjustment}`);

  return formatRuns(adjustedRuns(rule, after, count, calendar, adjustment));
}

/** Runs the command line and returns what to print, so it can be tested without a process. */
export function runCli(argv: readonly string[]): CliResult {
  const [command, ...args] = argv;
  try {
    switch (command) {
      case "next":
        return { code: 0, stdout: next(args), stderr: "" };
      case "describe": {
        if (args.length !== 1) throw new UsageError("describe needs exactly one rule");
        return { code: 0, stdout: `${describeRule(parseRule(args[0]))}\n`, stderr: "" };
      }
      case undefined:
      case "help":
      case "--help":
        return { code: 0, stdout: USAGE, stderr: "" };
      default:
        throw new UsageError(`unknown command: ${command}`);
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const usage = error instanceof UsageError ? `\n${USAGE}` : "";
    return { code: 2, stdout: "", stderr: `cadence: ${message}\n${usage}` };
  }
}
