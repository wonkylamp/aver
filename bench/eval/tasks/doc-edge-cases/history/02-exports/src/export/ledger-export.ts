// The "Ledger lines" download: one row per posting.
import type { Chart } from "../ledger/accounts.ts";
import type { Entry, Journal, Posting } from "../ledger/journal.ts";
import type { Column } from "./csv.ts";
import { toCsv } from "./csv.ts";

interface Line {
  entry: Entry;
  posting: Posting;
  accountName: string;
}

export const LEDGER_COLUMNS: Column<Line>[] = [
  { label: "Date", type: "date", value: (l) => l.entry.date },
  { label: "Entry", type: "text", value: (l) => l.entry.id },
  { label: "Reference", type: "text", value: (l) => l.entry.reference },
  { label: "Payee", type: "text", value: (l) => l.entry.payee },
  { label: "Account", type: "text", value: (l) => l.posting.account },
  { label: "Account name", type: "text", value: (l) => l.accountName },
  { label: "Memo", type: "text", value: (l) => l.posting.memo },
  { label: "Debit", type: "amount", value: (l) => (l.posting.cents > 0 ? l.posting.cents : null) },
  { label: "Credit", type: "amount", value: (l) => (l.posting.cents < 0 ? -l.posting.cents : null) },
  { label: "Net", type: "amount", value: (l) => l.posting.cents },
  { label: "Reconciled", type: "boolean", value: (l) => l.entry.reconciled },
  { label: "Posted at", type: "timestamp", value: (l) => l.entry.postedAt },
];

export function ledgerLines(chart: Chart, journal: Journal, range: { from?: Date; to?: Date } = {}): Line[] {
  return journal.entries(range).flatMap((entry) => entry.postings.map((posting) => ({ entry, posting, accountName: chart.get(posting.account).name })));
}

export function exportLedger(chart: Chart, journal: Journal, range: { from?: Date; to?: Date } = {}): string {
  return toCsv(LEDGER_COLUMNS, ledgerLines(chart, journal, range));
}
