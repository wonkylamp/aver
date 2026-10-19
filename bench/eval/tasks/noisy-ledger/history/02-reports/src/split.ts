// Splitting a transaction across categories (a supermarket shop that was
// part groceries, part household).
import type { Ledger, Transaction } from "./ledger.ts";
import { allocate } from "./money.ts";

export class SplitError extends Error {
  override name = "SplitError";
}

export interface Part {
  category: string;
  /** Minor units with the transaction's sign, or a share of the total. */
  amount?: number;
  share?: number;
}

/** Amounts for each part: fixed amounts first, shares split the rest. */
export function splitAmounts(total: number, parts: Part[]): number[] {
  if (parts.length < 2) throw new SplitError("a split needs at least two parts");
  const fixed = parts.reduce((n, p) => n + (p.amount ?? 0), 0);
  const rest = total - fixed;
  const shares = parts.filter((p) => p.amount === undefined);
  if (shares.length === 0) {
    if (rest !== 0) throw new SplitError(`parts add up to ${fixed}, not ${total}`);
    return parts.map((p) => p.amount!);
  }
  const weights = shares.map((p) => p.share ?? 1);
  const sum = weights.reduce((a, b) => a + b, 0);
  if (weights.every((w) => w === weights[0])) {
    const even = allocate(rest, shares.length);
    let k = 0;
    return parts.map((p) => p.amount ?? even[k++]!);
  }
  let given = 0;
  const amounts = weights.map((w, i) => {
    const a = i === weights.length - 1 ? rest - given : Math.round((rest * w) / sum);
    given += a;
    return a;
  });
  let k = 0;
  return parts.map((p) => p.amount ?? amounts[k++]!);
}

export function splitTransaction(ledger: Ledger, id: string, parts: Part[]): Transaction[] {
  const t = ledger.get(id);
  if (!t) throw new SplitError(`no transaction ${id}`);
  const amounts = splitAmounts(t.amount, parts);
  ledger.remove(id);
  return parts.map((p, i) => ledger.add({ account: t.account, date: t.date, amount: amounts[i]!, payee: t.payee, memo: t.memo, category: p.category, tags: [...t.tags, "split"], source: t.source }));
}
