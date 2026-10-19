// Overtime: billable time past 40 hours per person per ISO week
// (docs/billing.md, "Overtime").
import { isoWeek } from "./weeks.ts";

export interface OvertimeRule {
  weeklyHours: number;
  multiplier: number;
}

export const OVERTIME: OvertimeRule = { weeklyHours: 40, multiplier: 1.5 };

export interface Billed<E> {
  entry: E;
  /** Billable (rounded) minutes. */
  minutes: number;
}

export interface Split<E> extends Billed<E> {
  regularMinutes: number;
  overtimeMinutes: number;
}

/**
 * Splits billable time into regular and overtime, per person per ISO week,
 * in start order; the entry that crosses the line is split.
 */
export function splitOvertime<E extends { person: string; start: Date }>(billed: Billed<E>[], rule: OvertimeRule = OVERTIME): Split<E>[] {
  const limit = rule.weeklyHours * 60;
  const used = new Map<string, number>();
  const order = [...billed].sort((a, b) => a.entry.start.getTime() - b.entry.start.getTime());
  const out = new Map<Billed<E>, Split<E>>();
  for (const b of order) {
    const key = `${b.entry.person} ${isoWeek(b.entry.start)}`;
    const before = used.get(key) ?? 0;
    const regular = Math.max(0, Math.min(b.minutes, limit - before));
    used.set(key, before + b.minutes);
    out.set(b, { ...b, regularMinutes: regular, overtimeMinutes: b.minutes - regular });
  }
  return billed.map((b) => out.get(b)!);
}
