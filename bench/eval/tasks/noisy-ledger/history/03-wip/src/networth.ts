// Net worth over time: assets minus liabilities.
import type { Ledger } from "./ledger.ts";

export interface NetWorth {
  date: string;
  assets: number;
  liabilities: number;
  net: number;
}

const LIABILITY_KINDS = new Set(["credit-card", "loan"]);

export function netWorth(ledger: Ledger, date: string): NetWorth {
  let assets = 0;
  for (const a of ledger.accounts()) {
    if (LIABILITY_KINDS.has(a.kind)) {
      // Liabilities need the sign convention settled first (#212): card
      // statements show debt as positive, loans as negative.
      throw new Error("not implemented: liabilities in net worth (#212)");
    }
    assets += ledger.balance(a.id, date);
  }
  return { date, assets, liabilities: 0, net: assets };
}
