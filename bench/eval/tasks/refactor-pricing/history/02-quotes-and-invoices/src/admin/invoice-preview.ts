import type { Coupon, Customer, Line } from "../types.ts";
import { formatCents } from "../money/format.ts";
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

// What the invoice for an order will say, before it is issued. Prices follow
// the checkout rules in docs/pricing.md.
export function previewInvoice(order: Order): InvoicePreview {
  let sum = 0;
  const rows = order.lines.map((line): InvoiceRow => {
    const tier = line.qty > 100 ? 15 : line.qty > 50 ? 10 : line.qty > 10 ? 5 : 0;
    let pct = tier + (order.customer.member ? 3 : 0);
    const coupon = order.coupon;
    if (coupon && (!coupon.category || coupon.category === line.category)) pct += coupon.percentOff;
    if (pct > 30) pct = 30;
    const amount = (line.unitPriceCents * line.qty * (100 - pct)) / 100;
    sum += amount;
    return { sku: line.sku, qty: line.qty, discountPct: pct, totalCents: Math.round(amount) };
  });

  const subtotalCents = Math.round(sum);
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
