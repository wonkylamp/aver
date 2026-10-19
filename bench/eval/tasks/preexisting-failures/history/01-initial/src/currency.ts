export type CurrencyCode = "EUR" | "GBP" | "USD" | "SEK" | "JPY";

export type Currency = {
  code: CurrencyCode;
  /** Digits after the decimal point; amounts are stored in this unit. */
  minorUnits: number;
  symbol: string;
  /** True when the symbol is written after the number ("12.50 kr"). */
  symbolAfter: boolean;
};

const CURRENCIES: Record<CurrencyCode, Currency> = {
  EUR: { code: "EUR", minorUnits: 2, symbol: "€", symbolAfter: false },
  GBP: { code: "GBP", minorUnits: 2, symbol: "£", symbolAfter: false },
  USD: { code: "USD", minorUnits: 2, symbol: "$", symbolAfter: false },
  SEK: { code: "SEK", minorUnits: 2, symbol: "kr", symbolAfter: true },
  JPY: { code: "JPY", minorUnits: 0, symbol: "¥", symbolAfter: false },
};

export function isCurrencyCode(code: string): code is CurrencyCode {
  return Object.hasOwn(CURRENCIES, code);
}

export function currency(code: string): Currency {
  const upper = code.trim().toUpperCase();
  if (!isCurrencyCode(upper)) throw new RangeError(`unsupported currency: ${code}`);
  return CURRENCIES[upper];
}

export function supportedCurrencies(): CurrencyCode[] {
  return (Object.keys(CURRENCIES) as CurrencyCode[]).sort();
}
