import { percentOf } from "./money.ts";
import type { Bps, Cents } from "./money.ts";

export type Region = "DE" | "FR" | "GB" | "IE" | "NL";
export type TaxCategory = "standard" | "reduced" | "zero" | "exempt";

/**
 * VAT rates in basis points by region and category. "zero" and "exempt" are
 * both 0% but are reported separately to the tax office.
 */
const RATES: Record<Region, Record<TaxCategory, Bps>> = {
  DE: { standard: 1900, reduced: 700, zero: 0, exempt: 0 },
  FR: { standard: 2000, reduced: 550, zero: 0, exempt: 0 },
  GB: { standard: 2000, reduced: 500, zero: 0, exempt: 0 },
  IE: { standard: 2300, reduced: 1350, zero: 0, exempt: 0 },
  NL: { standard: 2100, reduced: 900, zero: 0, exempt: 0 },
};

export function isRegion(value: string): value is Region {
  return Object.hasOwn(RATES, value);
}

export function isTaxCategory(value: string): value is TaxCategory {
  return value === "standard" || value === "reduced" || value === "zero" || value === "exempt";
}

export function taxRate(region: Region, category: TaxCategory): Bps {
  return RATES[region][category];
}

/** Tax on one line: the taxable amount times the rate, rounded half up to a cent. */
export function lineTax(taxable: Cents, rate: Bps): Cents {
  return percentOf(taxable, rate);
}

/** "19%", "5.5%", "0%". */
export function formatRate(rate: Bps): string {
  const whole = Math.floor(rate / 100);
  const fraction = rate % 100;
  if (fraction === 0) return `${whole}%`;
  return `${whole}.${String(fraction).padStart(2, "0").replace(/0$/, "")}%`;
}
