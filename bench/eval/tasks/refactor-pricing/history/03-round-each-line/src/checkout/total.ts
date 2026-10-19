import type { Cart, Coupon, Customer } from "../types.ts";
import { shippingCents } from "../shipping/shipping.ts";
import { taxCents } from "../tax/rates.ts";

export interface CheckoutLine {
  sku: string;
  qty: number;
  totalCents: number;
}

export interface CheckoutTotal {
  lines: CheckoutLine[];
  subtotalCents: number;
  shippingCents: number;
  taxCents: number;
  totalCents: number;
}

// Highest tier first: a line gets the first tier it reaches.
const BULK_TIERS = [
  { minQty: 100, pct: 15 },
  { minQty: 50, pct: 10 },
  { minQty: 10, pct: 5 },
];
const MEMBER_PCT = 3;
const MAX_DISCOUNT_PCT = 30;

export function checkoutTotal(cart: Cart, customer: Customer, coupon?: Coupon): CheckoutTotal {
  if (cart.lines.length === 0) throw new Error("cart is empty");

  const lines: CheckoutLine[] = [];
  let subtotal = 0;
  for (const line of cart.lines) {
    let pct = 0;
    for (const tier of BULK_TIERS) {
      if (line.qty > tier.minQty) {
        pct = tier.pct;
        break;
      }
    }
    if (customer.member) pct += MEMBER_PCT;
    if (coupon !== undefined && (coupon.category === undefined || coupon.category === line.category)) {
      pct += coupon.percentOff;
    }
    pct = Math.min(pct, MAX_DISCOUNT_PCT);

    // Each line is rounded to the cent before it is added: the receipt prints
    // the line amounts, and they have to add up to the subtotal.
    const cents = Math.round((line.unitPriceCents * line.qty * (100 - pct)) / 100);
    lines.push({ sku: line.sku, qty: line.qty, totalCents: cents });
    subtotal += cents;
  }

  const shipping = shippingCents(subtotal, cart.region);
  const tax = taxCents(subtotal + shipping, cart.region);
  return {
    lines,
    subtotalCents: subtotal,
    shippingCents: shipping,
    taxCents: tax,
    totalCents: subtotal + shipping + tax,
  };
}
