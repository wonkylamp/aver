// "$1,234.56" for 123456. Negative amounts (refunds) get a leading minus.
export function formatCents(cents: number): string {
  if (!Number.isInteger(cents)) throw new Error(`not a whole number of cents: ${cents}`);
  const sign = cents < 0 ? "-" : "";
  const abs = Math.abs(cents);
  const dollars = Math.floor(abs / 100)
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const rest = (abs % 100).toString().padStart(2, "0");
  return `${sign}$${dollars}.${rest}`;
}

// Parses "12.34", "$1,234.5" or "7" into cents. Used by the admin import.
export function parseCents(text: string): number {
  const cleaned = text.trim().replace(/^\$/, "").replace(/,/g, "");
  const match = /^(\d+)(?:\.(\d{1,2}))?$/.exec(cleaned);
  if (match === null) throw new Error(`not an amount: ${text}`);
  return Number(match[1]) * 100 + Number((match[2] ?? "0").padEnd(2, "0"));
}
