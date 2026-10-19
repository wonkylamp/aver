// The timesheet CSV payroll imports every Friday (docs/billing.md, "Timesheet export").
import { billableSeconds, DEFAULT_POLICY } from "./rounding.ts";
import type { Client, Entry } from "./types.ts";

function cell(s: string): string {
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export function timesheetCsv(entries: Entry[], clients: Client[]): string {
  const byId = new Map(clients.map((c) => [c.id, c]));
  const rows = [["date", "person", "client", "role", "tracked_hours", "billable_hours", "note"].join(",")];
  const sorted = [...entries].sort((a, b) => a.start.getTime() - b.start.getTime() || a.id.localeCompare(b.id));
  for (const e of sorted) {
    const policy = byId.get(e.client)?.policy ?? DEFAULT_POLICY;
    const billable = e.billable ? billableSeconds(e.seconds, policy) : 0;
    rows.push(
      [e.start.toISOString().slice(0, 10), e.person, e.client, e.role, (e.seconds / 60).toFixed(2), (billable / 60).toFixed(2), cell(e.note ?? "")].join(","),
    );
  }
  return rows.join("\n") + "\n";
}
