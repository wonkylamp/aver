import { isCurrencyCode } from "./currency.ts";
import { lineAmount, validateLine } from "./line-items.ts";
import { sum } from "./money.ts";
import { isRegion, lineTax, taxRate } from "./tax.ts";
import type { CurrencyCode } from "./currency.ts";
import type { LineItem } from "./line-items.ts";
import type { Bps, Cents } from "./money.ts";
import type { Region, TaxCategory } from "./tax.ts";

export type InvoiceInput = {
  number: string;
  region: Region;
  currency: CurrencyCode;
  /** YYYY-MM-DD */
  issueDate: string;
  lines: LineItem[];
};

export type InvoiceLine = {
  sku: string;
  description: string;
  quantity: number;
  unitPrice: Cents;
  taxCategory: TaxCategory;
  /** quantity * unitPrice */
  amount: Cents;
  taxRate: Bps;
  tax: Cents;
};

export type Invoice = {
  number: string;
  region: Region;
  currency: CurrencyCode;
  issueDate: string;
  lines: InvoiceLine[];
  subtotal: Cents;
  taxTotal: Cents;
  total: Cents;
};

export class InvoiceError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "InvoiceError";
  }
}

/** Computes line amounts, tax and totals. See docs/billing.md for the rules. */
export function computeInvoice(input: InvoiceInput): Invoice {
  if (!isRegion(input.region)) throw new InvoiceError(`unknown region: ${String(input.region)}`);
  if (!isCurrencyCode(input.currency)) throw new InvoiceError(`unknown currency: ${String(input.currency)}`);
  if (input.lines.length === 0) throw new InvoiceError("an invoice needs at least one line");
  input.lines.forEach((line, index) => {
    const problems = validateLine(line);
    if (problems.length > 0) throw new InvoiceError(`line ${index + 1}: ${problems.join("; ")}`);
  });

  const lines = input.lines.map((line): InvoiceLine => {
    const amount = lineAmount(line);
    const rate = taxRate(input.region, line.taxCategory);
    return {
      sku: line.sku,
      description: line.description,
      quantity: line.quantity,
      unitPrice: line.unitPrice,
      taxCategory: line.taxCategory,
      amount,
      taxRate: rate,
      tax: lineTax(amount, rate),
    };
  });

  const subtotal = sum(lines.map((line) => line.amount));
  const taxTotal = sum(lines.map((line) => line.tax));
  return {
    number: input.number,
    region: input.region,
    currency: input.currency,
    issueDate: input.issueDate,
    lines,
    subtotal,
    taxTotal,
    total: subtotal + taxTotal,
  };
}

/** Tax per rate, for the VAT breakdown printed under the lines. */
export function taxBreakdown(invoice: Invoice): { rate: Bps; tax: Cents }[] {
  const byRate = new Map<Bps, Cents>();
  for (const line of invoice.lines) {
    byRate.set(line.taxRate, (byRate.get(line.taxRate) ?? 0) + line.tax);
  }
  return [...byRate.entries()]
    .map(([rate, tax]) => ({ rate, tax }))
    .sort((a, b) => b.rate - a.rate);
}
