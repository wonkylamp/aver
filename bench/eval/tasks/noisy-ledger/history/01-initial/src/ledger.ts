// The ledger: accounts and their transactions.
import { newId } from "./ids.ts";

export type AccountKind = "current" | "savings" | "credit-card" | "loan" | "cash";

export interface Account {
  id: string;
  name: string;
  kind: AccountKind;
  currency: string;
  openingBalance: number;
}

export interface Transaction {
  id: string;
  account: string;
  date: string;
  /** Minor units; negative is money out. */
  amount: number;
  /** What the user sees; they can rename it. */
  payee: string;
  memo?: string;
  category?: string;
  tags: string[];
  /** Where it came from: an import keeps the bank's own fields here, unchanged. */
  source: { kind: "manual" } | { kind: "import"; bankId?: string; payee: string; statement: string };
}

export class LedgerError extends Error {
  override name = "LedgerError";
}

export type NewTransaction = Omit<Transaction, "id" | "tags"> & { tags?: string[] };

export class Ledger {
  readonly #accounts = new Map<string, Account>();
  readonly #txs: Transaction[] = [];

  addAccount(account: Account): void {
    if (this.#accounts.has(account.id)) throw new LedgerError(`duplicate account ${account.id}`);
    this.#accounts.set(account.id, account);
  }

  account(id: string): Account {
    const a = this.#accounts.get(id);
    if (!a) throw new LedgerError(`no account ${id}`);
    return a;
  }

  accounts(): Account[] {
    return [...this.#accounts.values()];
  }

  add(tx: NewTransaction): Transaction {
    this.account(tx.account);
    if (!Number.isInteger(tx.amount)) throw new LedgerError(`amounts are whole minor units: ${tx.amount}`);
    const t: Transaction = { ...tx, id: newId(tx.date), tags: tx.tags ?? [] };
    this.#txs.push(t);
    return t;
  }

  get(id: string): Transaction | undefined {
    return this.#txs.find((t) => t.id === id);
  }

  update(id: string, patch: Partial<Pick<Transaction, "payee" | "memo" | "category" | "tags">>): Transaction {
    const t = this.get(id);
    if (!t) throw new LedgerError(`no transaction ${id}`);
    Object.assign(t, patch);
    return t;
  }

  remove(id: string): void {
    const i = this.#txs.findIndex((t) => t.id === id);
    if (i < 0) throw new LedgerError(`no transaction ${id}`);
    this.#txs.splice(i, 1);
  }

  /** In date order, then insertion order. */
  transactions(filter: { account?: string; from?: string; to?: string } = {}): Transaction[] {
    return this.#txs
      .filter((t) => (filter.account === undefined || t.account === filter.account) && (filter.from === undefined || t.date >= filter.from) && (filter.to === undefined || t.date <= filter.to))
      .map((t, i) => [t, i] as const)
      .sort(([a, i], [b, j]) => a.date.localeCompare(b.date) || i - j)
      .map(([t]) => t);
  }

  /** Balance at the end of `date` (inclusive). */
  balance(account: string, date = "9999-12-31"): number {
    return this.transactions({ account, to: date }).reduce((n, t) => n + t.amount, this.account(account).openingBalance);
  }
}
