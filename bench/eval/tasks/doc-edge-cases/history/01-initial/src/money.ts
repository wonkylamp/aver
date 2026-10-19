// Amounts are integer cents everywhere; floats appear only at the edges.

export class MoneyError extends Error {
  override name = "MoneyError";
}

/** Parses "1234.5", "-0.05", "1,234.50" or "(12.00)" (accounting negative) into cents. */
export function parseAmount(text: string): number {
  let s = text.trim().replace(/,/g, "");
  let sign = 1;
  if (/^\(.*\)$/.test(s)) {
    sign = -1;
    s = s.slice(1, -1);
  }
  if (s.startsWith("-")) {
    sign = -sign;
    s = s.slice(1);
  }
  const m = /^(\d+)(?:\.(\d{1,2}))?$/.exec(s);
  if (!m) throw new MoneyError(`not an amount: ${JSON.stringify(text)}`);
  const cents = Number(m[1]) * 100 + Number((m[2] ?? "0").padEnd(2, "0"));
  return sign * cents;
}

/** For people: "1,234.50", "-0.05". */
export function formatAmount(cents: number): string {
  if (!Number.isInteger(cents)) throw new MoneyError(`not whole cents: ${cents}`);
  return (cents / 100).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

/** For people, with the currency: "EUR 1,234.50". */
export function formatMoney(cents: number, currency: string): string {
  return `${currency} ${formatAmount(cents)}`;
}

export function sum(values: number[]): number {
  return values.reduce((a, b) => a + b, 0);
}
