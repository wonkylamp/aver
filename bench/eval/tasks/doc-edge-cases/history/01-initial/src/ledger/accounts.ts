export type AccountType = "asset" | "liability" | "equity" | "income" | "expense";

export interface Account {
  code: string;
  name: string;
  type: AccountType;
}

export class ChartError extends Error {
  override name = "ChartError";
}

/** Debit-normal accounts grow with debits; the others with credits. */
export function debitNormal(type: AccountType): boolean {
  return type === "asset" || type === "expense";
}

export class Chart {
  readonly #accounts = new Map<string, Account>();

  constructor(accounts: Account[] = []) {
    for (const a of accounts) this.add(a);
  }

  add(account: Account): void {
    if (!/^\d{4}$/.test(account.code)) throw new ChartError(`account codes are four digits: ${account.code}`);
    if (this.#accounts.has(account.code)) throw new ChartError(`duplicate account ${account.code}`);
    this.#accounts.set(account.code, account);
  }

  get(code: string): Account {
    const a = this.#accounts.get(code);
    if (!a) throw new ChartError(`no account ${code}`);
    return a;
  }

  has(code: string): boolean {
    return this.#accounts.has(code);
  }

  all(): Account[] {
    return [...this.#accounts.values()].sort((a, b) => a.code.localeCompare(b.code));
  }
}

export const STANDARD_CHART: Account[] = [
  { code: "1000", name: "Cash", type: "asset" },
  { code: "1100", name: "Accounts receivable", type: "asset" },
  { code: "2000", name: "Accounts payable", type: "liability" },
  { code: "2100", name: "VAT payable", type: "liability" },
  { code: "3000", name: "Owner's equity", type: "equity" },
  { code: "4000", name: "Sales", type: "income" },
  { code: "5000", name: "Purchases", type: "expense" },
  { code: "6100", name: "Rent", type: "expense" },
  { code: "6200", name: "Software", type: "expense" },
];
