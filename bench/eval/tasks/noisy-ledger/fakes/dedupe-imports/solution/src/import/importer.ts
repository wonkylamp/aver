// Importing a bank statement into an account (docs/importing.md).
import type { Ledger, Transaction } from "../ledger.ts";
import type { StatementCache } from "./cache.ts";
import type { BankFormat, StatementRow } from "./formats.ts";
import { readStatement } from "./formats.ts";
import { displayPayee, normalizePayee } from "./payee.ts";

export interface ImportResult {
  added: Transaction[];
  /** Rows not imported, and why. */
  skipped: Array<{ row: StatementRow; reason: "pending" | "reverted" | "duplicate" }>;
}

export interface ImportOptions {
  format?: BankFormat;
  currency?: string;
  /** A name for the statement, kept on each transaction (usually the file name). */
  statement?: string;
  /** Reuses the rows parsed for the preview. */
  cache?: StatementCache;
}

/** Date, amount and the bank's payee as normalizePayee compares it. */
export function fingerprint(date: string, amount: number, bankPayee: string): string {
  return `${date}\u0000${amount}\u0000${normalizePayee(bankPayee)}`;
}

export function importRows(ledger: Ledger, account: string, rows: StatementRow[], statement = "statement"): ImportResult {
  const result: ImportResult = { added: [], skipped: [] };
  const knownIds = new Set<string>();
  // How many transactions of the account have each fingerprint, before this import.
  const inAccount = new Map<string, number>();
  for (const t of ledger.transactions({ account })) {
    if (t.source.kind !== "import") continue;
    if (t.source.bankId) knownIds.add(t.source.bankId);
    const fp = fingerprint(t.date, t.amount, t.source.payee);
    inAccount.set(fp, (inAccount.get(fp) ?? 0) + 1);
  }
  const inStatement = new Map<string, number>();
  for (const row of rows) {
    if (row.state !== "booked") {
      result.skipped.push({ row, reason: row.state });
      continue;
    }
    if (row.bankId !== undefined) {
      if (knownIds.has(row.bankId)) {
        result.skipped.push({ row, reason: "duplicate" });
        continue;
      }
      knownIds.add(row.bankId);
    } else {
      // Identical rows are real transactions: the first k of them are the k already in the account.
      const fp = fingerprint(row.date, row.amount, row.payee);
      const nth = (inStatement.get(fp) ?? 0) + 1;
      inStatement.set(fp, nth);
      if (nth <= (inAccount.get(fp) ?? 0)) {
        result.skipped.push({ row, reason: "duplicate" });
        continue;
      }
    }
    result.added.push(
      ledger.add({
        account,
        date: row.date,
        amount: row.amount,
        payee: displayPayee(row.payee),
        memo: row.memo,
        source: { kind: "import", bankId: row.bankId, payee: row.payee, statement },
      }),
    );
  }
  return result;
}

export function importStatement(ledger: Ledger, account: string, text: string, options: ImportOptions = {}): ImportResult {
  const currency = options.currency ?? ledger.account(account).currency;
  const rows = options.cache ? options.cache.read(text, options.format, currency) : readStatement(text, options.format, currency);
  return importRows(ledger, account, rows, options.statement);
}
