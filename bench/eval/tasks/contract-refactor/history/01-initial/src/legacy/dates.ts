// Date helpers. Dates are calendar dates in UTC.

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** "2026-03-14" for a Date, read in UTC. */
export function isoDate(d: Date): string {
  if (Number.isNaN(d.getTime())) throw new RangeError("invalid date");
  return `${d.getUTCFullYear()}-${pad2(d.getUTCMonth() + 1)}-${pad2(d.getUTCDate())}`;
}

/** `n` business days after `d` (Saturdays and Sundays skipped); a negative `n` goes back. */
export function addBusinessDays(d: Date, n: number): Date {
  const out = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()));
  const step = n < 0 ? -1 : 1;
  let left = Math.abs(n);
  while (left > 0) {
    out.setUTCDate(out.getUTCDate() + step);
    const day = out.getUTCDay();
    if (day !== 0 && day !== 6) left--;
  }
  return out;
}

/** "today", "tomorrow", "yesterday", "in 3 days", "5 days ago", by calendar day in UTC. */
export function relativeDays(d: Date, now: Date): string {
  const day = (x: Date) => Date.UTC(x.getUTCFullYear(), x.getUTCMonth(), x.getUTCDate()) / 86_400_000;
  const diff = day(d) - day(now);
  if (diff === 0) return "today";
  if (diff === 1) return "tomorrow";
  if (diff === -1) return "yesterday";
  return diff > 0 ? `in ${diff} days` : `${-diff} days ago`;
}
