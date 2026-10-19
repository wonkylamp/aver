import type { TaxClass } from "./catalog.ts";

export interface ShippingRule {
  /** Flat rate per order, minor units. */
  flatCents: number;
  /** Extra per started kilogram above the first. */
  perKgCents: number;
  /** Merchandise at or above this ships free. */
  freeOverCents?: number;
}

export interface Promo {
  code: string;
  kind: "percent" | "fixed";
  /** Percent off, or minor units off. */
  value: number;
  minSpendCents?: number;
}

/** One storefront: where it sells, in what currency, and its rules. */
export interface Store {
  id: string;
  country: string;
  currency: string;
  /** Percent by tax class. */
  taxRates: Record<TaxClass, number>;
  shipping: ShippingRule;
  promos?: Promo[];
  /** Prices and shipping include tax. */
  taxInclusive?: boolean;
  /** Totals are paid in multiples of this many minor units. */
  cashRounding?: number;
}

export const STORES: Record<string, Store> = {
  uk: {
    id: "uk",
    country: "GB",
    currency: "GBP",
    taxRates: { standard: 20, reduced: 5, zero: 0 },
    shipping: { flatCents: 395, perKgCents: 150, freeOverCents: 5000 },
    promos: [
      { code: "SPRING10", kind: "percent", value: 10 },
      { code: "TAKE5", kind: "fixed", value: 500, minSpendCents: 2500 },
    ],
  },
  us: { id: "us", country: "US", currency: "USD", taxRates: { standard: 8.875, reduced: 4.5, zero: 0 }, shipping: { flatCents: 599, perKgCents: 200 } },
  de: { id: "de", country: "DE", currency: "EUR", taxRates: { standard: 19, reduced: 7, zero: 0 }, shipping: { flatCents: 490, perKgCents: 100, freeOverCents: 6000 }, taxInclusive: true },
  ch: { id: "ch", country: "CH", currency: "CHF", taxRates: { standard: 8.1, reduced: 2.6, zero: 0 }, shipping: { flatCents: 700, perKgCents: 250, freeOverCents: 8000 }, taxInclusive: true, cashRounding: 5 },
};
