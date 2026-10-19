// Monthly spending by category, and the top payees.
import type { Transaction } from "./ledger.ts";

export interface CategoryTotal {
  category: string;
  spent: number;
  count: number;
}

export function spendingByCategory(txs: Transaction[], month: string): CategoryTotal[] {
  const totals = new Map<string, CategoryTotal>();
  for (const t of txs) {
    if (!t.date.startsWith(month) || t.amount >= 0) continue;
    const category = t.category ?? "uncategorised";
    const c = totals.get(category) ?? { category, spent: 0, count: 0 };
    c.spent -= t.amount;
    c.count += 1;
    totals.set(category, c);
  }
  return [...totals.values()].sort((a, b) => b.spent - a.spent || a.category.localeCompare(b.category));
}

export function topPayees(txs: Transaction[], month: string, n = 5): Array<{ payee: string; spent: number }> {
  const totals = new Map<string, number>();
  for (const t of txs) if (t.date.startsWith(month) && t.amount < 0) totals.set(t.payee, (totals.get(t.payee) ?? 0) - t.amount);
  return [...totals]
    .map(([payee, spent]) => ({ payee, spent }))
    .sort((a, b) => b.spent - a.spent || a.payee.localeCompare(b.payee))
    .slice(0, n);
}

export function incomeAndSpending(txs: Transaction[], month: string): { income: number; spending: number; net: number } {
  let income = 0;
  let spending = 0;
  for (const t of txs) {
    if (!t.date.startsWith(month) || t.category === "transfer") continue;
    if (t.amount > 0) income += t.amount;
    else spending -= t.amount;
  }
  return { income, spending, net: income - spending };
}
