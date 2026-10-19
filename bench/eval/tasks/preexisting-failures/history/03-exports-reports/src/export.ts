import { toCsv } from "./csv.ts";
import { currency } from "./currency.ts";
import { formatDecimal } from "./format.ts";
import type { Invoice } from "./invoice.ts";

const LINE_HEADER = [
  "invoice",
  "issue_date",
  "currency",
  "sku",
  "description",
  "quantity",
  "unit_price",
  "amount",
  "discount",
  "tax_rate_bps",
  "tax",
];

/** One row per invoice line, amounts as plain decimals, for the ledger import. */
export function invoiceLinesCsv(invoices: readonly Invoice[]): string {
  const rows = invoices.flatMap((invoice) => {
    const units = currency(invoice.currency).minorUnits;
    return invoice.lines.map((line) => [
      invoice.number,
      invoice.issueDate,
      invoice.currency,
      line.sku,
      line.description,
      line.quantity,
      formatDecimal(line.unitPrice, units),
      formatDecimal(line.amount, units),
      formatDecimal(line.discount, units),
      line.taxRate,
      formatDecimal(line.tax, units),
    ]);
  });
  return toCsv(LINE_HEADER, rows);
}

const SUMMARY_HEADER = ["invoice", "issue_date", "currency", "subtotal", "discount_code", "discount", "tax", "total"];

/** One row per invoice. */
export function invoiceSummaryCsv(invoices: readonly Invoice[]): string {
  const rows = invoices.map((invoice) => {
    const units = currency(invoice.currency).minorUnits;
    return [
      invoice.number,
      invoice.issueDate,
      invoice.currency,
      formatDecimal(invoice.subtotal, units),
      invoice.discountCode ?? "",
      formatDecimal(invoice.discountTotal, units),
      formatDecimal(invoice.taxTotal, units),
      formatDecimal(invoice.total, units),
    ];
  });
  return toCsv(SUMMARY_HEADER, rows);
}
