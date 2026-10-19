import type { Coupon, Customer, Line } from "../types.ts";
import { addDays } from "../dates.ts";
import { priceLines } from "../pricing/engine.ts";

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

export function buildQuote(request: QuoteRequest): Quote {
  if (request.lines.length === 0) throw new Error("a quote needs at least one line");
  const validDays = request.validDays ?? 30;
  if (!Number.isInteger(validDays) || validDays < 1) throw new Error(`validDays must be a positive whole number`);

  const priced = priceLines(request.lines, { member: request.customer.member, coupon: request.coupon });
  return {
    customerId: request.customer.id,
    lines: priced.lines.map((line) => ({
      sku: line.sku,
      qty: line.qty,
      unitPriceCents: line.unitPriceCents,
      discountPct: line.discountPct,
      totalCents: line.totalCents,
    })),
    subtotalCents: priced.subtotalCents,
    issuedOn: request.issuedOn,
    expiresOn: addDays(request.issuedOn, validDays),
  };
}
