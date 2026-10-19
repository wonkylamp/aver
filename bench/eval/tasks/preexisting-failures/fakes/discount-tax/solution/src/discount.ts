import { compareIsoDates } from "./dates.ts";
import { percentOf } from "./money.ts";
import type { IsoDate } from "./dates.ts";
import type { Bps, Cents } from "./money.ts";

/**
 * A discount code applies to the whole invoice. Percent codes take a share of
 * the subtotal; fixed codes (gift vouchers, goodwill credits) take an amount.
 */
export type Discount =
  | { code: string; kind: "percent"; rate: Bps; validUntil?: IsoDate }
  | { code: string; kind: "fixed"; amount: Cents; validUntil?: IsoDate };

export type DiscountCatalog = ReadonlyMap<string, Discount>;

export class DiscountError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DiscountError";
  }
}

export function normalizeCode(code: string): string {
  return code.trim().toUpperCase().replace(/\s+/g, "");
}

export function makeCatalog(discounts: readonly Discount[]): DiscountCatalog {
  const catalog = new Map<string, Discount>();
  for (const discount of discounts) {
    const code = normalizeCode(discount.code);
    if (catalog.has(code)) throw new DiscountError(`duplicate discount code: ${code}`);
    if (discount.kind === "percent" && (discount.rate <= 0 || discount.rate > 10_000)) {
      throw new DiscountError(`${code}: percent discounts must be between 0 and 100%`);
    }
    if (discount.kind === "fixed" && (!Number.isSafeInteger(discount.amount) || discount.amount <= 0)) {
      throw new DiscountError(`${code}: fixed discounts must be a positive whole amount`);
    }
    catalog.set(code, { ...discount, code });
  }
  return catalog;
}

/** Finds a code as typed by a customer, rejecting unknown and expired codes. */
export function lookupDiscount(catalog: DiscountCatalog, typed: string, on: IsoDate): Discount {
  const code = normalizeCode(typed);
  const discount = catalog.get(code);
  if (discount === undefined) throw new DiscountError(`unknown discount code: ${code}`);
  if (discount.validUntil !== undefined && compareIsoDates(on, discount.validUntil) > 0) {
    throw new DiscountError(`discount code ${code} expired on ${discount.validUntil}`);
  }
  return discount;
}

/** The amount a discount takes off a subtotal. Fixed amounts are capped at the subtotal. */
export function discountAmount(discount: Discount, subtotal: Cents): Cents {
  if (discount.kind === "percent") return percentOf(subtotal, discount.rate);
  return Math.min(discount.amount, subtotal);
}
