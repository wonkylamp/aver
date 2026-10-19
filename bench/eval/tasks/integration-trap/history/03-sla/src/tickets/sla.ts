// First-response SLA: how long a ticket may wait for its first agent reply,
// counted in business hours (Monday to Friday, 09:00-17:00 UTC).
import type { Ticket } from "./store.ts";

export type Plan = "standard" | "priority";

const TARGET_HOURS: Record<Plan, number> = { standard: 16, priority: 4 };
const OPEN_HOUR = 9;
const CLOSE_HOUR = 17;

function isBusinessDay(d: Date): boolean {
  const day = d.getUTCDay();
  return day !== 0 && day !== 6;
}

/** Business milliseconds between `from` and `to`. */
export function businessMs(from: Date, to: Date): number {
  if (to <= from) return 0;
  let total = 0;
  const day = new Date(Date.UTC(from.getUTCFullYear(), from.getUTCMonth(), from.getUTCDate()));
  while (day < to) {
    if (isBusinessDay(day)) {
      const open = day.getTime() + OPEN_HOUR * 3_600_000;
      const close = day.getTime() + CLOSE_HOUR * 3_600_000;
      total += Math.max(0, Math.min(close, to.getTime()) - Math.max(open, from.getTime()));
    }
    day.setUTCDate(day.getUTCDate() + 1);
  }
  return total;
}

export interface SlaState {
  targetHours: number;
  elapsedHours: number;
  breached: boolean;
  firstResponseAt: Date | null;
}

export function firstResponseSla(ticket: Ticket, plan: Plan, now: Date): SlaState {
  const first = ticket.messages.find((m) => m.kind === "agent");
  const end = first ? first.at : now;
  const elapsedHours = businessMs(ticket.createdAt, end) / 3_600_000;
  const targetHours = TARGET_HOURS[plan];
  return { targetHours, elapsedHours: Math.round(elapsedHours * 100) / 100, breached: elapsedHours > targetHours, firstResponseAt: first ? first.at : null };
}
