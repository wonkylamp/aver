// Categorisation rules: the first matching rule, by priority, gives a
// transaction its category. Built-in rules plus the user's, registered at
// runtime (the settings screen registers and unregisters them).

export interface RuleMatch {
  /** Tested against the display payee. */
  payee?: RegExp;
  /** Money out at least this much (minor units, positive). */
  outAtLeast?: number;
  /** Money in only. */
  income?: boolean;
  account?: string;
}

export interface Rule {
  id: string;
  match: RuleMatch;
  category: string;
  /** Higher runs first; built-in rules are 0-10. */
  priority: number;
}

export interface Categorizable {
  payee: string;
  amount: number;
  account: string;
}

export const BUILTIN_RULES: Rule[] = [
  { id: "salary", match: { income: true, payee: /payroll|salary|wages/i }, category: "income", priority: 10 },
  { id: "refund", match: { income: true }, category: "refunds", priority: 1 },
  { id: "groceries", match: { payee: /tesco|sainsbury|asda|aldi|lidl|waitrose|co-?op/i }, category: "groceries", priority: 5 },
  { id: "coffee", match: { payee: /coffee|caf[eé]|pret|starbucks|costa/i }, category: "eating-out", priority: 5 },
  { id: "transport", match: { payee: /\btfl\b|trainline|uber|bolt|national rail/i }, category: "transport", priority: 5 },
  { id: "bills", match: { payee: /octopus|british gas|thames water|council tax|bt group|vodafone/i }, category: "bills", priority: 5 },
  { id: "rent", match: { payee: /rent|letting/i, outAtLeast: 50000 }, category: "rent", priority: 6 },
  { id: "subscriptions", match: { payee: /netflix|spotify|disney|apple\.com|youtube premium/i }, category: "subscriptions", priority: 5 },
];

const registered: Rule[] = [];

/** Adds a rule; returns the function that removes it. */
export function registerRule(rule: Rule): () => void {
  if (registered.some((r) => r.id === rule.id) || BUILTIN_RULES.some((r) => r.id === rule.id)) throw new Error(`duplicate rule ${rule.id}`);
  registered.push(rule);
  return () => {
    const i = registered.indexOf(rule);
    if (i >= 0) registered.splice(i, 1);
  };
}

export function activeRules(): Rule[] {
  return [...registered, ...BUILTIN_RULES].sort((a, b) => b.priority - a.priority);
}

export function matches(rule: Rule, tx: Categorizable): boolean {
  const m = rule.match;
  if (m.payee && !m.payee.test(tx.payee)) return false;
  if (m.income !== undefined && m.income !== tx.amount > 0) return false;
  if (m.outAtLeast !== undefined && !(tx.amount < 0 && -tx.amount >= m.outAtLeast)) return false;
  if (m.account !== undefined && m.account !== tx.account) return false;
  return true;
}

export function categorize(tx: Categorizable, rules: Rule[] = activeRules()): string | undefined {
  return rules.find((r) => matches(r, tx))?.category;
}
