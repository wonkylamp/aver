// Transactions for tests, without a ledger.
import type { Transaction } from "../ledger.ts";

let n = 0;

export function tx(date: string, amount: number, payee: string, extra: Partial<Transaction> = {}): Transaction {
  return { id: `t${++n}`, account: "cur", date, amount, payee, tags: [], source: { kind: "manual" }, ...extra };
}
