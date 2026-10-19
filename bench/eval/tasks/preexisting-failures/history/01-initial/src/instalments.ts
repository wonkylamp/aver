import { addDays } from "./dates.ts";
import { allocate } from "./money.ts";
import type { IsoDate } from "./dates.ts";
import type { Cents } from "./money.ts";

export type Instalment = { dueDate: IsoDate; amount: Cents };

/**
 * Splits an invoice total into `count` instalments due every `intervalDays`
 * days from `firstDue`. Instalments differ by at most one cent; the earlier
 * ones carry the extra cents.
 */
export function planInstalments(total: Cents, count: number, firstDue: IsoDate, intervalDays: number): Instalment[] {
  if (!Number.isSafeInteger(count) || count < 1) throw new RangeError(`invalid instalment count: ${count}`);
  const amounts = allocate(total, Array.from({ length: count }, () => 1));
  return amounts.map((amount, index) => ({ dueDate: addDays(firstDue, index * intervalDays), amount }));
}
