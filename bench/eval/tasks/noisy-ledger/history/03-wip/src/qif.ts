// QIF export, for desktop finance programs that still only read QIF.
import type { Account, Transaction } from "./ledger.ts";
import { formatAmount } from "./money.ts";

const TYPES: Record<Account["kind"], string> = { current: "Bank", savings: "Bank", cash: "Cash", "credit-card": "CCard", loan: "Oth L" };

/** QIF dates are US order. */
export function qifDate(date: string): string {
  return `${date.slice(5, 7)}/${date.slice(8, 10)}/${date.slice(0, 4)}`;
}

export function toQif(account: Account, txs: Transaction[]): string {
  const lines = [`!Type:${TYPES[account.kind]}`];
  for (const t of txs) {
    lines.push(`D${qifDate(t.date)}`, `T${formatAmount(t.amount, account.currency).replace(/,/g, "")}`, `P${t.payee}`);
    if (t.memo) lines.push(`M${t.memo}`);
    if (t.category) lines.push(`L${t.category}`);
    lines.push("^");
  }
  return lines.join("\n") + "\n";
}
