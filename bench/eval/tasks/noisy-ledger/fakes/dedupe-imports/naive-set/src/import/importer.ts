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

export function importRows(ledger: Ledger, account: string, rows: StatementRow[], statement = "statement"): ImportResult {
  const result: ImportResult = { added: [], skipped: [] };
  const knownIds = new Set(
    ledger
      .transactions({ account })
      .flatMap((t) => (t.source.kind === "import" && t.source.bankId ? [t.source.bankId] : [])),
  );
  const key = (date: string, amount: number, payee: string) => `${date}|${amount}|${normalizePayee(payee)}`;
  const seen = new Set(
    ledger
      .transactions({ account })
      .flatMap((t) => (t.source.kind === "import" ? [key(t.date, t.amount, t.source.payee)] : [])),
  );
  for (const row of rows) {
    if (row.state !== "booked") {
      result.skipped.push({ row, reason: row.state });
      continue;
    }
    if (row.bankId !== undefined && knownIds.has(row.bankId)) {
      result.skipped.push({ row, reason: "duplicate" });
      continue;
    }
    if (row.bankId === undefined) {
      const k = key(row.date, row.amount, row.payee);
      if (seen.has(k)) {
        result.skipped.push({ row, reason: "duplicate" });
        continue;
      }
      seen.add(k);
    }
    if (row.bankId !== undefined) knownIds.add(row.bankId);
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
