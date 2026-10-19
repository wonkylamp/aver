// A client's invoice for a period (docs/billing.md, "Invoices").
import { hours } from "./duration.ts";
import { splitOvertime } from "./overtime.ts";
import { billableSeconds, DEFAULT_POLICY } from "./rounding.ts";
import type { Client, Entry, Role } from "./types.ts";

export interface InvoiceLine {
  role: Role;
  regularHours: string;
  overtimeHours: string;
  rateCents: number;
  amountCents: number;
}

export interface Invoice {
  client: string;
  lines: InvoiceLine[];
  subtotalCents: number;
  vatCents: number;
  totalCents: number;
}

const ROLES: Role[] = ["engineer", "designer", "manager"];

export function buildInvoice(client: Client, entries: Entry[], period: { from: Date; to: Date }): Invoice {
  const mine = entries.filter((e) => e.client === client.id && e.billable && e.start >= period.from && e.start < period.to);
  const policy = client.policy ?? DEFAULT_POLICY;
  const billed = mine.map((entry) => ({ entry, seconds: billableSeconds(entry.seconds, policy) }));
  const split = client.overtime ? splitOvertime(billed) : billed.map((b) => ({ ...b, regularSeconds: b.seconds, overtimeSeconds: 0 }));
  const lines: InvoiceLine[] = [];
  for (const role of ROLES) {
    const of = split.filter((s) => s.entry.role === role);
    if (of.length === 0) continue;
    const regular = of.reduce((n, s) => n + s.regularSeconds, 0);
    const overtime = of.reduce((n, s) => n + s.overtimeSeconds, 0);
    const rate = client.rates[role];
    const amountCents = Math.round((regular * rate + overtime * rate * 1.5) / 3600);
    lines.push({ role, regularHours: hours(regular), overtimeHours: hours(overtime), rateCents: rate, amountCents });
  }
  const subtotalCents = lines.reduce((n, l) => n + l.amountCents, 0);
  const vatCents = Math.round((subtotalCents * client.vatRate) / 100);
  return { client: client.id, lines, subtotalCents, vatCents, totalCents: subtotalCents + vatCents };
}
