// Transaction search: words match the payee or memo, and filters narrow it.
//   coffee amount>3 category:eating-out after:2026-03-01 tag:work
import type { Transaction } from "./ledger.ts";

export interface Query {
  words: string[];
  category?: string;
  tag?: string;
  after?: string;
  before?: string;
  minAmount?: number;
  maxAmount?: number;
  account?: string;
}

export class QueryError extends Error {
  override name = "QueryError";
}

function units(text: string): number {
  if (!/^\d+(\.\d{1,2})?$/.test(text)) throw new QueryError(`not an amount: ${text}`);
  return Math.round(Number(text) * 100);
}

export function parseQuery(text: string): Query {
  const q: Query = { words: [] };
  for (const token of text.trim().split(/\s+/).filter(Boolean)) {
    let m: RegExpExecArray | null;
    if ((m = /^(category|tag|account):(.+)$/.exec(token))) q[m[1] as "category" | "tag" | "account"] = m[2]!.toLowerCase();
    else if ((m = /^(after|before):(\d{4}-\d{2}-\d{2})$/.exec(token))) q[m[1] as "after" | "before"] = m[2]!;
    else if ((m = /^amount([<>])(.+)$/.exec(token))) {
      if (m[1] === ">") q.minAmount = units(m[2]!);
      else q.maxAmount = units(m[2]!);
    } else if (/^\w+:/.test(token)) throw new QueryError(`unknown filter ${token}`);
    else q.words.push(token.toLowerCase());
  }
  return q;
}

function text(t: Transaction): string {
  return `${t.payee} ${t.memo ?? ""}`.toLowerCase();
}

export function search(txs: Transaction[], query: string | Query): Transaction[] {
  const q = typeof query === "string" ? parseQuery(query) : query;
  return txs.filter((t) => {
    const size = Math.abs(t.amount);
    return (
      q.words.every((w) => text(t).includes(w)) &&
      (q.category === undefined || t.category === q.category) &&
      (q.tag === undefined || t.tags.includes(q.tag)) &&
      (q.account === undefined || t.account === q.account) &&
      (q.after === undefined || t.date > q.after) &&
      (q.before === undefined || t.date < q.before) &&
      (q.minAmount === undefined || size > q.minAmount) &&
      (q.maxAmount === undefined || size < q.maxAmount)
    );
  });
}
