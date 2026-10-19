import type { Cart, Coupon, Customer } from "../types.ts";
import { priceLines } from "../pricing/engine.ts";
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

export function checkoutTotal(cart: Cart, customer: Customer, coupon?: Coupon): CheckoutTotal {
  if (cart.lines.length === 0) throw new Error("cart is empty");

  const priced = priceLines(cart.lines, { member: customer.member, coupon });
  const subtotal = priced.subtotalCents;
  const shipping = shippingCents(subtotal, cart.region);
  const tax = taxCents(subtotal + shipping, cart.region);
  return {
    lines: priced.lines.map((line) => ({ sku: line.sku, qty: line.qty, totalCents: line.totalCents })),
    subtotalCents: subtotal,
    shippingCents: shipping,
    taxCents: tax,
    totalCents: subtotal + shipping + tax,
  };
}
