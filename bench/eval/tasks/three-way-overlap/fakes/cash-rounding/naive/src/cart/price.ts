// Prices a cart for a store: line amounts, shipping, tax, the total and
// cash rounding (docs/pricing.md).
import type { Product } from "../catalog.ts";
import { product } from "../catalog.ts";
import { roundHalfUp } from "../money.ts";
import type { Store } from "../store.ts";

export interface CartItem {
  sku: string;
  qty: number;
}

export interface PricedLine {
  sku: string;
  qty: number;
  unitCents: number;
  amountCents: number;
  taxRate: number;
  taxCents: number;
}

export interface Quote {
  currency: string;
  lines: PricedLine[];
  /** Sum of line amounts. */
  subtotalCents: number;
  shippingCents: number;
  /** Line tax plus shipping tax. */
  taxCents: number;
  totalCents: number;
  /** What cash rounding added to the total (negative when it rounded down). */
  roundingCents: number;
  /** The total after cash rounding: what the customer pays. */
  payableCents: number;
}

export class PricingError extends Error {
  override name = "PricingError";
}

function shippingFor(items: Array<{ p: Product; qty: number }>, merchandiseCents: number, store: Store): number {
  const grams = items.reduce((n, { p, qty }) => n + p.weightGrams * qty, 0);
  if (grams === 0) return 0;
  const rule = store.shipping;
  if (rule.freeOverCents !== undefined && merchandiseCents >= rule.freeOverCents) return 0;
  const extraKg = Math.max(0, Math.ceil(grams / 1000) - 1);
  return rule.flatCents + extraKg * rule.perKgCents;
}

export function priceCart(cart: CartItem[], store: Store): Quote {
  const items = cart.map((c) => {
    if (!Number.isInteger(c.qty) || c.qty < 1) throw new PricingError(`bad quantity ${c.qty} for ${c.sku}`);
    return { p: product(c.sku), qty: c.qty };
  });
  const lines: PricedLine[] = items.map(({ p, qty }) => {
    const amountCents = p.priceCents * qty;
    const taxRate = store.taxRates[p.taxClass];
    return { sku: p.sku, qty, unitCents: p.priceCents, amountCents, taxRate, taxCents: roundHalfUp((amountCents * taxRate) / 100) };
  });
  const subtotalCents = lines.reduce((n, l) => n + l.amountCents, 0);
  const merchandise = items.reduce((n, { p, qty }) => n + (p.giftCard ? 0 : p.priceCents * qty), 0);
  const shippingCents = shippingFor(items, merchandise, store);
  const shippingTax = roundHalfUp((shippingCents * store.taxRates.standard) / 100);
  const taxCents = lines.reduce((n, l) => n + l.taxCents, 0) + shippingTax;
  const totalCents = subtotalCents + shippingCents + taxCents;
  const step = store.cashRounding ?? 1;
  const payableCents = Math.floor(totalCents / step) * step;
  return { currency: store.currency, lines, subtotalCents, shippingCents, taxCents, totalCents, roundingCents: payableCents - totalCents, payableCents };
}
