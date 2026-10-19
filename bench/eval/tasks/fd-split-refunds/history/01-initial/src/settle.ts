// Settling up: transfers that bring every balance to zero.

export type Transfer = { from: string; to: string; pence: number };

export class SettleError extends Error {}

/**
 * Greedy: the largest debtor pays the largest creditor until both are
 * square, ties by name. Balances must add up to zero.
 */
export function settle(balances: Record<string, number>): Transfer[] {
  const total = Object.values(balances).reduce((a, b) => a + b, 0);
  if (total !== 0) throw new SettleError(`balances add up to ${total}, not zero`);
  const debtors = Object.entries(balances).filter(([, v]) => v < 0).map(([name, v]) => ({ name, left: -v }));
  const creditors = Object.entries(balances).filter(([, v]) => v > 0).map(([name, v]) => ({ name, left: v }));
  const out: Transfer[] = [];
  const byLeft = (a: { name: string; left: number }, b: { name: string; left: number }) => b.left - a.left || a.name.localeCompare(b.name);
  while (debtors.length > 0 && creditors.length > 0) {
    debtors.sort(byLeft);
    creditors.sort(byLeft);
    const d = debtors[0]!;
    const c = creditors[0]!;
    const pence = Math.min(d.left, c.left);
    out.push({ from: d.name, to: c.name, pence });
    d.left -= pence;
    c.left -= pence;
    if (d.left === 0) debtors.shift();
    if (c.left === 0) creditors.shift();
  }
  return out;
}
