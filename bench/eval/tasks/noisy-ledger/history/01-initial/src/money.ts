// Amounts are signed integer minor units (pence, cents). Negative is money out.

export class MoneyError extends Error {
  override name = "MoneyError";
}

export const MINOR_UNITS: Record<string, number> = { GBP: 2, EUR: 2, USD: 2, JPY: 0, CHF: 2, SEK: 2, BHD: 3 };

export function minorUnits(currency: string): number {
  const n = MINOR_UNITS[currency];
  if (n === undefined) throw new MoneyError(`unknown currency ${currency}`);
  return n;
}

/**
 * Parses an amount as banks print it: "-12.50", "12.50-", "(12.50)",
 * "1,234.56", "£4.20", "EUR 3" or, with `decimal: ","`, "1.234,56".
 */
export function parseAmount(text: string, currency = "GBP", decimal: "." | "," = "."): number {
  let s = text.trim().replace(/^[£$€]|^[A-Z]{3}\s+/, "").replace(/\s/g, "");
  let sign = 1;
  if (/^\(.*\)$/.test(s)) {
    sign = -1;
    s = s.slice(1, -1);
  } else if (s.endsWith("-")) {
    sign = -1;
    s = s.slice(0, -1);
  }
  if (s.startsWith("-")) {
    sign *= -1;
    s = s.slice(1);
  } else if (s.startsWith("+")) s = s.slice(1);
  const group = decimal === "." ? "," : ".";
  if (!new RegExp(`^\\d{1,3}(\\${group}\\d{3})*(\\${decimal}\\d+)?$|^\\d+(\\${decimal}\\d+)?$`).test(s)) {
    throw new MoneyError(`not an amount: ${JSON.stringify(text)}`);
  }
  const [whole = "0", frac = ""] = s.split(group).join("").split(decimal);
  const digits = minorUnits(currency);
  if (frac.length > digits) throw new MoneyError(`${text} has more than ${digits} decimals for ${currency}`);
  return sign * (Number(whole) * 10 ** digits + Number(frac.padEnd(digits, "0") || "0"));
}

export function formatAmount(minor: number, currency = "GBP"): string {
  const digits = minorUnits(currency);
  const abs = Math.abs(minor);
  const whole = Math.floor(abs / 10 ** digits).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const frac = digits ? `.${String(abs % 10 ** digits).padStart(digits, "0")}` : "";
  return `${minor < 0 ? "-" : ""}${whole}${frac}`;
}

export function formatMoney(minor: number, currency: string): string {
  return `${currency} ${formatAmount(minor, currency)}`;
}

/** Splits `minor` into `parts` amounts that differ by at most one unit, larger first. */
export function allocate(minor: number, parts: number): number[] {
  if (!Number.isInteger(parts) || parts < 1) throw new MoneyError(`cannot split into ${parts}`);
  const sign = minor < 0 ? -1 : 1;
  const abs = Math.abs(minor);
  const base = Math.floor(abs / parts);
  const extra = abs - base * parts;
  return Array.from({ length: parts }, (_, i) => sign * (base + (i < extra ? 1 : 0)));
}
