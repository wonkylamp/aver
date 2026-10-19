// A group, its expenses (refunds are negative expenses) and the balances.
import { splitByShares, splitEvenly } from "./split.ts";

export class GroupError extends Error {}

export type Expense = {
  description: string;
  paidBy: string;
  /** Pence; negative for a refund, which is paid back to `paidBy`. */
  amountPence: number;
  /** Who it was for, in order; everyone when absent. */
  among?: string[];
  /** Shares of `among`, when not even. */
  shares?: number[];
};

export class Group {
  readonly members: string[];
  readonly expenses: Expense[] = [];

  constructor(members: string[]) {
    if (members.length === 0 || new Set(members).size !== members.length) throw new GroupError("members must be distinct and at least one");
    this.members = [...members];
  }

  add(e: Expense): void {
    const among = e.among ?? this.members;
    for (const m of [e.paidBy, ...among]) if (!this.members.includes(m)) throw new GroupError(`${m} is not in the group`);
    if (e.shares && e.shares.length !== among.length) throw new GroupError("one share per person");
    this.expenses.push({ ...e, among: [...among] });
  }

  /** What each person's part of an expense is, in `among` order. */
  parts(e: Expense): number[] {
    const among = e.among ?? this.members;
    return e.shares ? splitByShares(e.amountPence, e.shares) : splitEvenly(e.amountPence, among.length);
  }

  /** Paid minus owed, per member: positive is owed money. They add up to zero. */
  balances(): Record<string, number> {
    const out: Record<string, number> = Object.fromEntries(this.members.map((m) => [m, 0]));
    for (const e of this.expenses) {
      out[e.paidBy]! += e.amountPence;
      const among = e.among ?? this.members;
      this.parts(e).forEach((p, i) => {
        out[among[i]!]! -= p;
      });
    }
    return out;
  }
}
