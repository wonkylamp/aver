import { isTaxCategory } from "./tax.ts";
import type { Cents } from "./money.ts";
import type { TaxCategory } from "./tax.ts";

export type LineItem = {
  sku: string;
  description: string;
  /** Whole units. */
  quantity: number;
  /** Price of one unit before tax, in minor units. */
  unitPrice: Cents;
  taxCategory: TaxCategory;
};

export function lineAmount(line: LineItem): Cents {
  return line.quantity * line.unitPrice;
}

/** Returns human-readable problems with a line; an empty list means it is valid. */
export function validateLine(line: LineItem): string[] {
  const problems: string[] = [];
  if (line.sku.trim() === "") problems.push("sku is empty");
  if (!Number.isSafeInteger(line.quantity) || line.quantity < 0) {
    problems.push(`quantity must be a non-negative whole number, got ${line.quantity}`);
  }
  if (!Number.isSafeInteger(line.unitPrice) || line.unitPrice < 0) {
    problems.push(`unitPrice must be a non-negative whole number of minor units, got ${line.unitPrice}`);
  }
  if (!isTaxCategory(line.taxCategory)) problems.push(`unknown tax category: ${String(line.taxCategory)}`);
  return problems;
}
