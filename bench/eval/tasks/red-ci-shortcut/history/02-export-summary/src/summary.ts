// The weekly summary managers get on Monday morning.
import { hours } from "./duration.ts";
import { splitOvertime } from "./overtime.ts";
import { billableMinutes, DEFAULT_POLICY } from "./rounding.ts";
import type { Client, Entry } from "./types.ts";
import { isoWeek } from "./weeks.ts";

export interface PersonWeek {
  person: string;
  week: string;
  trackedHours: string;
  billableHours: string;
  overtimeHours: string;
  nonBillableHours: string;
}

export function weeklySummary(entries: Entry[], clients: Client[]): PersonWeek[] {
  const byId = new Map(clients.map((c) => [c.id, c]));
  const billed = entries.filter((e) => e.billable).map((entry) => ({ entry, minutes: billableMinutes(entry.minutes, byId.get(entry.client)?.policy ?? DEFAULT_POLICY) }));
  const split = splitOvertime(billed);
  const rows = new Map<string, { person: string; week: string; tracked: number; billable: number; overtime: number; nonBillable: number }>();
  const row = (e: Entry) => {
    const key = `${e.person} ${isoWeek(e.start)}`;
    let r = rows.get(key);
    if (!r) rows.set(key, (r = { person: e.person, week: isoWeek(e.start), tracked: 0, billable: 0, overtime: 0, nonBillable: 0 }));
    return r;
  };
  for (const e of entries) {
    const r = row(e);
    r.tracked += e.minutes;
    if (!e.billable) r.nonBillable += e.minutes;
  }
  for (const s of split) {
    const r = row(s.entry);
    r.billable += s.minutes;
    r.overtime += s.overtimeMinutes;
  }
  return [...rows.values()]
    .sort((a, b) => a.week.localeCompare(b.week) || a.person.localeCompare(b.person))
    .map((r) => ({ person: r.person, week: r.week, trackedHours: hours(r.tracked), billableHours: hours(r.billable), overtimeHours: hours(r.overtime), nonBillableHours: hours(r.nonBillable) }));
}
