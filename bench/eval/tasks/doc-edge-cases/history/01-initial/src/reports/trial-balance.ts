import { formatAmount } from "../money.ts";
import type { Chart } from "../ledger/accounts.ts";
import type { Journal } from "../ledger/journal.ts";

export interface TrialBalanceRow {
  code: string;
  name: string;
  debit: number;
  credit: number;
}

export interface TrialBalance {
  rows: TrialBalanceRow[];
  totalDebit: number;
  totalCredit: number;
  balanced: boolean;
}

export function trialBalance(chart: Chart, journal: Journal, asOf: Date): TrialBalance {
  const net = new Map<string, number>();
  for (const e of journal.entries({ to: asOf })) {
    for (const p of e.postings) net.set(p.account, (net.get(p.account) ?? 0) + p.cents);
  }
  const rows: TrialBalanceRow[] = [];
  for (const a of chart.all()) {
    const n = net.get(a.code) ?? 0;
    if (n === 0) continue;
    rows.push({ code: a.code, name: a.name, debit: n > 0 ? n : 0, credit: n < 0 ? -n : 0 });
  }
  const totalDebit = rows.reduce((s, r) => s + r.debit, 0);
  const totalCredit = rows.reduce((s, r) => s + r.credit, 0);
  return { rows, totalDebit, totalCredit, balanced: totalDebit === totalCredit };
}

/** The trial balance as a fixed-width table for the console and emails. */
export function renderTrialBalance(tb: TrialBalance): string {
  const line = (code: string, name: string, dr: string, cr: string) => `${code.padEnd(6)}${name.padEnd(24)}${dr.padStart(14)}${cr.padStart(14)}`;
  const out = [line("Code", "Account", "Debit", "Credit")];
  for (const r of tb.rows) out.push(line(r.code, r.name, r.debit ? formatAmount(r.debit) : "", r.credit ? formatAmount(r.credit) : ""));
  out.push(line("", "Total", formatAmount(tb.totalDebit), formatAmount(tb.totalCredit)));
  return out.join("\n");
}
