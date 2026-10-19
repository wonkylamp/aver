import type { Coupon, Line } from "../types.ts";

// The line-pricing rules in docs/pricing.md. Checkout, quotes and the invoice
// preview all price through here so they agree to the cent.

export interface PricingContext {
  member: boolean;
  coupon?: Coupon;
}

export interface PricedLine {
  sku: string;
  qty: number;
  unitPriceCents: number;
  grossCents: number;
  /** Total discount in percent, after the cap. */
  discountPct: number;
  totalCents: number;
}

export interface PricedLines {
  lines: PricedLine[];
  subtotalCents: number;
}

const BULK_TIERS = [
  { minQty: 100, pct: 15 },
  { minQty: 50, pct: 10 },
  { minQty: 10, pct: 5 },
];
const MEMBER_PCT = 3;
const MAX_DISCOUNT_PCT = 30;

function bulkPct(qty: number): number {
  return BULK_TIERS.find((tier) => qty > tier.minQty)?.pct ?? 0;
}

function couponApplies(coupon: Coupon | undefined, line: Line): coupon is Coupon {
  return coupon !== undefined && (coupon.category === undefined || coupon.category === line.category);
}

export function priceLine(line: Line, ctx: PricingContext): PricedLine {
  let pct = bulkPct(line.qty);
  if (ctx.member) pct += MEMBER_PCT;
  if (couponApplies(ctx.coupon, line)) pct += ctx.coupon.percentOff;
  const discountPct = Math.min(pct, MAX_DISCOUNT_PCT);
  const grossCents = line.unitPriceCents * line.qty;
  return {
    sku: line.sku,
    qty: line.qty,
    unitPriceCents: line.unitPriceCents,
    grossCents,
    discountPct,
    totalCents: Math.round(amountCents(grossCents, discountPct)),
  };
}

function amountCents(grossCents: number, discountPct: number): number {
  return (grossCents * (100 - discountPct)) / 100;
}

export function priceLines(lines: Line[], ctx: PricingContext): PricedLines {
  const priced = lines.map((line) => priceLine(line, ctx));
  const sum = priced.reduce((total, line) => total + amountCents(line.grossCents, line.discountPct), 0);
  return { lines: priced, subtotalCents: Math.round(sum) };
}
