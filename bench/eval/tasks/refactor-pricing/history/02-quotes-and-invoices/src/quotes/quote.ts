import type { Coupon, Customer, Line } from "../types.ts";
import { addDays } from "../dates.ts";

export interface QuoteRequest {
  customer: Customer;
  lines: Line[];
  coupon?: Coupon;
  /** ISO date the quote is issued on, e.g. "2026-03-01". */
  issuedOn: string;
  /** Days the quoted prices hold. Defaults to 30. */
  validDays?: number;
}

export interface QuoteLine {
  sku: string;
  qty: number;
  unitPriceCents: number;
  /** Total discount on the line, in percent. */
  discountPct: number;
  totalCents: number;
}

export interface Quote {
  customerId: string;
  lines: QuoteLine[];
  subtotalCents: number;
  issuedOn: string;
  expiresOn: string;
}

// Line pricing, the same rules as checkout (docs/pricing.md), so a quote shows
// what the customer will pay when they order.
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

function lineDiscountPct(line: Line, member: boolean, coupon: Coupon | undefined): number {
  let pct = bulkPct(line.qty);
  if (member) pct += MEMBER_PCT;
  if (couponApplies(coupon, line)) pct += coupon.percentOff;
  return Math.min(pct, MAX_DISCOUNT_PCT);
}

function lineAmount(line: Line, discountPct: number): number {
  return (line.unitPriceCents * line.qty * (100 - discountPct)) / 100;
}

export function buildQuote(request: QuoteRequest): Quote {
  if (request.lines.length === 0) throw new Error("a quote needs at least one line");
  const validDays = request.validDays ?? 30;
  if (!Number.isInteger(validDays) || validDays < 1) throw new Error(`validDays must be a positive whole number`);

  let sum = 0;
  const lines = request.lines.map((line): QuoteLine => {
    const discountPct = lineDiscountPct(line, request.customer.member, request.coupon);
    const amount = lineAmount(line, discountPct);
    sum += amount;
    return {
      sku: line.sku,
      qty: line.qty,
      unitPriceCents: line.unitPriceCents,
      discountPct,
      totalCents: Math.round(amount),
    };
  });

  return {
    customerId: request.customer.id,
    lines,
    subtotalCents: Math.round(sum),
    issuedOn: request.issuedOn,
    expiresOn: addDays(request.issuedOn, validDays),
  };
}
