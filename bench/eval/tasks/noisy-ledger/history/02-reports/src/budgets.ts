// Monthly budgets per category, with optional rollover of what was left.
import type { Transaction } from "./ledger.ts";

export interface Budget {
  category: string;
  /** Per month, minor units, positive. */
  limit: number;
  /** Unspent (or overspent) amounts carry into the next month. */
  rollover: boolean;
}

export interface BudgetMonth {
  category: string;
  month: string;
  limit: number;
  carried: number;
  spent: number;
  left: number;
}

export function spentIn(txs: Transaction[], category: string, month: string): number {
  return txs.filter((t) => t.category === category && t.date.startsWith(month) && t.amount < 0).reduce((n, t) => n - t.amount, 0);
}

function nextMonth(month: string): string {
  const [y, m] = month.split("-").map(Number) as [number, number];
  return `${y}-${String(m + 1).padStart(2, "0")}`;
}

/** The budget's months from `from` to `to` (inclusive, "YYYY-MM"). */
export function budgetMonths(budget: Budget, txs: Transaction[], from: string, to: string): BudgetMonth[] {
  const out: BudgetMonth[] = [];
  let carried = 0;
  for (let month = from; month <= to; month = nextMonth(month)) {
    const spent = spentIn(txs, budget.category, month);
    const left = budget.limit + carried - spent;
    out.push({ category: budget.category, month, limit: budget.limit, carried, spent, left });
    carried = budget.rollover ? left : 0;
    if (out.length > 120) break;
  }
  return out;
}

export function overBudget(months: BudgetMonth[]): BudgetMonth[] {
  return months.filter((m) => m.left < 0);
}
