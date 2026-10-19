// The "Vendors" download.
import type { Vendor } from "../vendors.ts";
import type { Column } from "./csv.ts";
import { toCsv } from "./csv.ts";

export const VENDOR_COLUMNS: Column<Vendor>[] = [
  { label: "Vendor", type: "text", value: (v) => v.name },
  { label: "Email", type: "text", value: (v) => v.email },
  { label: "Phone", type: "text", value: (v) => v.phone },
  { label: "VAT number", type: "text", value: (v) => v.vatNumber },
  { label: "Terms (days)", type: "number", value: (v) => v.paymentTermsDays },
  { label: "Active", type: "boolean", value: (v) => v.active },
  { label: "Balance", type: "amount", value: (v) => v.balanceCents },
  { label: "Last paid", type: "timestamp", value: (v) => v.lastPaidAt },
];

export function exportVendors(vendors: Vendor[]): string {
  return toCsv(VENDOR_COLUMNS, [...vendors].sort((a, b) => a.name.localeCompare(b.name, "en")));
}
