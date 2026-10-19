import { formatMoney } from "./format.ts";
import type { CurrencyCode } from "./currency.ts";
import type { Invoice } from "./invoice.ts";
import type { Cents } from "./money.ts";

export type RevenueRow = {
  /** YYYY-MM */
  month: string;
  currency: CurrencyCode;
  invoices: number;
  /** subtotal minus discounts */
  net: Cents;
  tax: Cents;
  gross: Cents;
};

/** Revenue per calendar month and currency, oldest month first. */
export function monthlyRevenue(invoices: readonly Invoice[]): RevenueRow[] {
  const rows = new Map<string, RevenueRow>();
  for (const invoice of invoices) {
    const month = invoice.issueDate.slice(0, 7);
    const key = `${month} ${invoice.currency}`;
    const row = rows.get(key) ?? { month, currency: invoice.currency, invoices: 0, net: 0, tax: 0, gross: 0 };
    row.invoices += 1;
    row.net += invoice.subtotal - invoice.discountTotal;
    row.tax += invoice.taxTotal;
    row.gross += invoice.total;
    rows.set(key, row);
  }
  return [...rows.values()].sort((a, b) => a.month.localeCompare(b.month) || a.currency.localeCompare(b.currency));
}

/** A fixed-width text table for the monthly finance email. */
export function formatRevenueTable(rows: readonly RevenueRow[]): string {
  const header = ["Month", "Cur", "Invoices", "Net", "Tax", "Gross"];
  const body = rows.map((row) => [
    row.month,
    row.currency,
    String(row.invoices),
    formatMoney(row.net, row.currency),
    formatMoney(row.tax, row.currency),
    formatMoney(row.gross, row.currency),
  ]);
  const widths = header.map((title, column) => Math.max(title.length, ...body.map((cells) => cells[column].length)));
  const render = (cells: string[]) =>
    cells.map((cell, column) => (column < 2 ? cell.padEnd(widths[column]) : cell.padStart(widths[column]))).join("  ");
  return [render(header), ...body.map(render)].join("\n");
}

export type QuarterRow = {
  /** "2024-Q1" */
  quarter: string;
  currency: CurrencyCode;
  invoices: number;
  net: Cents;
  tax: Cents;
  gross: Cents;
};

/**
 * Revenue per calendar quarter and currency (docs/exports.md). Lands with the
 * quarterly VAT filing work (#230); the test describes the expected output.
 */
export function quarterlySummary(invoices: readonly Invoice[]): QuarterRow[] {
  void invoices;
  throw new Error("quarterlySummary is not implemented yet");
}
