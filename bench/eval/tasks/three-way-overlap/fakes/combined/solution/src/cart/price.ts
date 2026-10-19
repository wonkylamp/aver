// Prices a cart for a store: line amounts, promo discount, shipping, tax,
// the total and cash rounding (docs/pricing.md).
import type { Product } from "../catalog.ts";
import { product } from "../catalog.ts";
import { allocate, roundHalfUp } from "../money.ts";
import type { Promo, Store } from "../store.ts";

export interface CartItem {
  sku: string;
  qty: number;
}

export interface PricedLine {
  sku: string;
  qty: number;
  unitCents: number;
  amountCents: number;
  discountCents: number;
  taxRate: number;
  taxCents: number;
}

export interface Quote {
  currency: string;
  lines: PricedLine[];
  /** Sum of line amounts, before the discount. */
  subtotalCents: number;
  discountCents: number;
  shippingCents: number;
  /** Line tax plus shipping tax (included in the prices in a tax-inclusive store). */
  taxCents: number;
  totalCents: number;
  roundingCents: number;
  payableCents: number;
}

export interface PriceOptions {
  promo?: string;
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

function findPromo(store: Store, code: string): Promo {
  const promo = (store.promos ?? []).find((p) => p.code.toLowerCase() === code.trim().toLowerCase());
  if (!promo) throw new PricingError(`unknown promo code ${code}`);
  return promo;
}

/** Each line's share of the promo discount. */
function discounts(items: Array<{ p: Product; amount: number }>, promo: Promo | undefined): number[] {
  if (!promo) return items.map(() => 0);
  const eligible = items.map(({ p, amount }) => (p.giftCard ? 0 : amount));
  const base = eligible.reduce((a, b) => a + b, 0);
  if (promo.minSpendCents !== undefined && base < promo.minSpendCents) {
    throw new PricingError(`${promo.code} needs a minimum spend of ${promo.minSpendCents}`);
  }
  if (promo.kind === "percent") return eligible.map((a) => roundHalfUp((a * promo.value) / 100));
  return allocate(Math.min(promo.value, base), eligible);
}

function tax(amount: number, rate: number, inclusive: boolean): number {
  return roundHalfUp(inclusive ? (amount * rate) / (100 + rate) : (amount * rate) / 100);
}

export function priceCart(cart: CartItem[], store: Store, options: PriceOptions = {}): Quote {
  const items = cart.map((c) => {
    if (!Number.isInteger(c.qty) || c.qty < 1) throw new PricingError(`bad quantity ${c.qty} for ${c.sku}`);
    const p = product(c.sku);
    return { p, qty: c.qty, amount: p.priceCents * c.qty };
  });
  const promo = options.promo === undefined ? undefined : findPromo(store, options.promo);
  const off = discounts(items, promo);
  const inclusive = store.taxInclusive === true;
  const lines: PricedLine[] = items.map(({ p, qty, amount }, i) => {
    const taxRate = store.taxRates[p.taxClass];
    return { sku: p.sku, qty, unitCents: p.priceCents, amountCents: amount, discountCents: off[i]!, taxRate, taxCents: tax(amount - off[i]!, taxRate, inclusive) };
  });
  const subtotalCents = lines.reduce((n, l) => n + l.amountCents, 0);
  const discountCents = lines.reduce((n, l) => n + l.discountCents, 0);
  const merchandise = items.reduce((n, { p, amount }, i) => n + (p.giftCard ? 0 : amount - off[i]!), 0);
  const shippingCents = shippingFor(items, merchandise, store);
  const taxCents = lines.reduce((n, l) => n + l.taxCents, 0) + tax(shippingCents, store.taxRates.standard, inclusive);
  const totalCents = subtotalCents - discountCents + shippingCents + (inclusive ? 0 : taxCents);
  const step = store.cashRounding ?? 1;
  const payableCents = roundHalfUp(totalCents / step) * step;
  return { currency: store.currency, lines, subtotalCents, discountCents, shippingCents, taxCents, totalCents, roundingCents: payableCents - totalCents, payableCents };
}
