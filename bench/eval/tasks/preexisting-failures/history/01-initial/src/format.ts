import { currency } from "./currency.ts";
import type { CurrencyCode } from "./currency.ts";
import type { Cents } from "./money.ts";

/** "1234.50" for 123450 with two minor units; "1235" for JPY. No grouping. */
export function formatDecimal(amount: Cents, minorUnits: number): string {
  const negative = amount < 0;
  const digits = String(Math.abs(amount)).padStart(minorUnits + 1, "0");
  const whole = minorUnits === 0 ? digits : digits.slice(0, -minorUnits);
  const fraction = minorUnits === 0 ? "" : `.${digits.slice(-minorUnits)}`;
  return `${negative ? "-" : ""}${whole}${fraction}`;
}

function group(whole: string): string {
  return whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** "€1,234.50", "-£3.00", "12.50 kr", "¥1,235". */
export function formatMoney(amount: Cents, code: CurrencyCode): string {
  const info = currency(code);
  const plain = formatDecimal(Math.abs(amount), info.minorUnits);
  const [whole, fraction] = plain.split(".");
  const number = fraction === undefined ? group(whole) : `${group(whole)}.${fraction}`;
  const sign = amount < 0 ? "-" : "";
  return info.symbolAfter ? `${sign}${number} ${info.symbol}` : `${sign}${info.symbol}${number}`;
}
