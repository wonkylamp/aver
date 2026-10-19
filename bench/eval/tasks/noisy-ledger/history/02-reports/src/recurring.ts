// Recurring payments: the same payee charging about the same amount on
// about the same day every month (subscriptions, bills).
import { daysBetween } from "./dates.ts";
import { normalizePayee } from "./import/payee.ts";
import type { Transaction } from "./ledger.ts";

export interface Recurring {
  payee: string;
  amount: number;
  dayOfMonth: number;
  months: number;
  next: string;
}

/** Billing days may drift by this many days either way (weekends, bank holidays). */
export const DAY_TOLERANCE = 2;
/** Amounts may vary by this fraction (price rises, usage-based bills). */
export const AMOUNT_TOLERANCE = 0.1;

export function findRecurring(txs: Transaction[], minMonths = 3): Recurring[] {
  const byPayee = new Map<string, Transaction[]>();
  for (const t of txs) {
    if (t.amount >= 0) continue;
    const key = normalizePayee(t.payee);
    byPayee.set(key, [...(byPayee.get(key) ?? []), t]);
  }
  const out: Recurring[] = [];
  for (const list of byPayee.values()) {
    const sorted = [...list].sort((a, b) => a.date.localeCompare(b.date));
    const first = sorted[0]!;
    const day = Number(first.date.slice(8, 10));
    let ok = sorted.length >= minMonths;
    for (let i = 1; ok && i < sorted.length; i++) {
      const gap = daysBetween(sorted[i - 1]!.date, sorted[i]!.date);
      const drift = Math.abs(Number(sorted[i]!.date.slice(8, 10)) - day);
      if (gap < 25 || gap > 36 || drift > DAY_TOLERANCE) ok = false;
      if (Math.abs(sorted[i]!.amount - first.amount) > Math.abs(first.amount) * AMOUNT_TOLERANCE) ok = false;
    }
    if (!ok) continue;
    const last = sorted.at(-1)!;
    const y = Number(last.date.slice(0, 4));
    const m = Number(last.date.slice(5, 7));
    const next = `${m === 12 ? y + 1 : y}-${String(m === 12 ? 1 : m + 1).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
    out.push({ payee: last.payee, amount: last.amount, dayOfMonth: day, months: sorted.length, next });
  }
  return out.sort((a, b) => a.payee.localeCompare(b.payee));
}
