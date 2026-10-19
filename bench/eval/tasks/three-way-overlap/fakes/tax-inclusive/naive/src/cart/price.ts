// Prices a cart for a store: line amounts, shipping, tax and the total
// (docs/pricing.md).
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
  /** Line tax plus shipping tax (included in the prices in a tax-inclusive store). */
  taxCents: number;
  totalCents: number;
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

function tax(amount: number, rate: number, inclusive: boolean): number {
  return roundHalfUp(inclusive ? (amount * rate) / (100 + rate) : (amount * rate) / 100);
}

export function priceCart(cart: CartItem[], store: Store): Quote {
  const items = cart.map((c) => {
    if (!Number.isInteger(c.qty) || c.qty < 1) throw new PricingError(`bad quantity ${c.qty} for ${c.sku}`);
    const p = product(c.sku);
    return { p, qty: c.qty, amount: p.priceCents * c.qty };
  });
  const inclusive = store.taxInclusive === true;
  const lines: PricedLine[] = items.map(({ p, qty, amount }) => {
    const taxRate = store.taxRates[p.taxClass];
    return { sku: p.sku, qty, unitCents: p.priceCents, amountCents: amount, taxRate, taxCents: tax(amount, taxRate, inclusive) };
  });
  const subtotalCents = lines.reduce((n, l) => n + l.amountCents, 0);
  const merchandise = items.reduce((n, { p, amount }) => n + (p.giftCard ? 0 : amount), 0);
  const shippingCents = shippingFor(items, merchandise, store);
  const taxCents = lines.reduce((n, l) => n + l.taxCents, 0) + tax(shippingCents, store.taxRates.standard, false);
  const totalCents = subtotalCents + shippingCents + (inclusive ? tax(shippingCents, store.taxRates.standard, false) : taxCents);
  return { currency: store.currency, lines, subtotalCents, shippingCents, taxCents, totalCents };
}
