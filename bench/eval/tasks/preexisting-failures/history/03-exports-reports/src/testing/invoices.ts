import { computeInvoice } from "../invoice.ts";
import type { Invoice } from "../invoice.ts";

/** Small invoices without discount codes, shared by the export and revenue tests. */
export function sampleInvoices(): Invoice[] {
  return [
    computeInvoice({
      number: "INV-2024-0001",
      region: "DE",
      currency: "EUR",
      issueDate: "2024-01-20",
      lines: [
        { sku: "LAMP-01", description: "Desk lamp", quantity: 2, unitPrice: 2_499, taxCategory: "standard" },
        { sku: "BOOK-17", description: "Travel guide", quantity: 1, unitPrice: 1_800, taxCategory: "reduced" },
      ],
    }),
    computeInvoice({
      number: "INV-2024-0002",
      region: "GB",
      currency: "GBP",
      issueDate: "2024-02-03",
      lines: [{ sku: "SEAT-02", description: "Child car seat", quantity: 1, unitPrice: 8_000, taxCategory: "reduced" }],
    }),
    computeInvoice({
      number: "INV-2024-0003",
      region: "DE",
      currency: "EUR",
      issueDate: "2024-02-28",
      lines: [{ sku: "DESK-04", description: "Standing desk, oak", quantity: 1, unitPrice: 45_000, taxCategory: "standard" }],
    }),
    computeInvoice({
      number: "INV-2024-0004",
      region: "FR",
      currency: "EUR",
      issueDate: "2024-04-02",
      lines: [{ sku: "LAMP-01", description: "Desk lamp", quantity: 1, unitPrice: 2_499, taxCategory: "standard" }],
    }),
  ];
}
