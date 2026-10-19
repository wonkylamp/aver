import type { Coupon, Customer, Line } from "../types.ts";
import { formatCents } from "../money/format.ts";
import { priceLines } from "../pricing/engine.ts";
import { shippingCents } from "../shipping/shipping.ts";
import { taxCents } from "../tax/rates.ts";

export interface Order {
  id: string;
  region: string;
  customer: Customer;
  lines: Line[];
  coupon?: Coupon;
}

export interface InvoiceRow {
  sku: string;
  qty: number;
  discountPct: number;
  totalCents: number;
}

export interface InvoicePreview {
  orderId: string;
  rows: InvoiceRow[];
  subtotalCents: number;
  shippingCents: number;
  taxCents: number;
  totalCents: number;
  /** Plain-text rendering for the back-office screen and the email draft. */
  text: string;
}

// What the invoice for an order will say, before it is issued.
export function previewInvoice(order: Order): InvoicePreview {
  const priced = priceLines(order.lines, { member: order.customer.member, coupon: order.coupon });
  const rows = priced.lines.map(
    (line): InvoiceRow => ({ sku: line.sku, qty: line.qty, discountPct: line.discountPct, totalCents: line.totalCents }),
  );

  const subtotalCents = priced.subtotalCents;
  const shipping = shippingCents(subtotalCents, order.region);
  const tax = taxCents(subtotalCents + shipping, order.region);
  const totalCents = subtotalCents + shipping + tax;
  return {
    orderId: order.id,
    rows,
    subtotalCents,
    shippingCents: shipping,
    taxCents: tax,
    totalCents,
    text: render(order, rows, subtotalCents, shipping, tax, totalCents),
  };
}

function render(order: Order, rows: InvoiceRow[], subtotal: number, shipping: number, tax: number, total: number): string {
  const out = [`Invoice preview for order ${order.id} (customer ${order.customer.id})`, ""];
  for (const row of rows) {
    const discount = row.discountPct > 0 ? `-${row.discountPct}%` : "";
    out.push(`${row.sku.padEnd(12)}${`x${row.qty}`.padStart(6)}${discount.padStart(6)}${formatCents(row.totalCents).padStart(14)}`);
  }
  out.push("");
  out.push(`${"Subtotal".padEnd(24)}${formatCents(subtotal).padStart(14)}`);
  out.push(`${"Shipping".padEnd(24)}${formatCents(shipping).padStart(14)}`);
  out.push(`${"Tax".padEnd(24)}${formatCents(tax).padStart(14)}`);
  out.push(`${"Total".padEnd(24)}${formatCents(total).padStart(14)}`);
  return out.join("\n");
}
