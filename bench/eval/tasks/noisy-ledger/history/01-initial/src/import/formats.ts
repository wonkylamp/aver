// Bank statement dialects, read into one row shape.
import { parseCsvObjects } from "../csv.ts";
import { parseDate } from "../dates.ts";
import { parseAmount } from "../money.ts";

export type BankFormat = "monzo" | "barclays" | "revolut" | "starling";

export interface StatementRow {
  date: string;
  amount: number;
  payee: string;
  memo?: string;
  /** Only some banks give each transaction an id of its own (Monzo does). */
  bankId?: string;
  state: "booked" | "pending" | "reverted";
}

export class FormatError extends Error {
  override name = "FormatError";
}

function need(row: Record<string, string>, key: string, line: number): string {
  const v = row[key];
  if (v === undefined) throw new FormatError(`line ${line}: no ${key} column`);
  return v;
}

export function detectFormat(text: string): BankFormat {
  const header = (text.replace(/^\uFEFF/, "").split(/\r?\n/, 1)[0] ?? "").toLowerCase();
  if (header.startsWith("transaction id,")) return "monzo";
  if (header.startsWith("number,date,account,amount")) return "barclays";
  if (header.startsWith("type,product,started date")) return "revolut";
  if (header.startsWith("date,counter party")) return "starling";
  throw new FormatError("unrecognised statement format");
}

export function readStatement(text: string, format: BankFormat = detectFormat(text), currency = "GBP"): StatementRow[] {
  const rows = parseCsvObjects(text);
  return rows.map((r, i): StatementRow => {
    const line = i + 2;
    switch (format) {
      case "monzo":
        return {
          bankId: need(r, "Transaction ID", line),
          date: parseDate(need(r, "Date", line), "DD/MM/YYYY"),
          payee: need(r, "Name", line),
          amount: parseAmount(need(r, "Amount", line), currency),
          memo: r.Notes || undefined,
          state: "booked",
        };
      case "barclays":
        return {
          date: parseDate(need(r, "Date", line), "DD/MM/YYYY"),
          payee: need(r, "Memo", line).split("\t")[0]!.trim(),
          amount: parseAmount(need(r, "Amount", line), currency),
          state: "booked",
        };
      case "revolut": {
        const state = need(r, "State", line).toUpperCase();
        const fee = r.Fee ? parseAmount(r.Fee, currency) : 0;
        return {
          date: parseDate(need(r, "Started Date", line).slice(0, 10)),
          payee: need(r, "Description", line),
          amount: parseAmount(need(r, "Amount", line), currency) - fee,
          state: state === "PENDING" ? "pending" : state === "REVERTED" ? "reverted" : "booked",
        };
      }
      case "starling":
        return {
          date: parseDate(need(r, "Date", line), "DD/MM/YYYY"),
          payee: need(r, "Counter Party", line),
          memo: r.Reference || undefined,
          amount: parseAmount(need(r, "Amount (GBP)", line), "GBP"),
          state: "booked",
        };
    }
  });
}
