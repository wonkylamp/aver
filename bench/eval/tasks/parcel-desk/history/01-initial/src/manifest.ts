// The daily manifest the carrier collects with the parcels
// (docs/shipping.md, "Manifest").
import { normalizePostcode } from "./address.ts";
import { toCsv } from "./csv.ts";
import type { Shipment } from "./shipment.ts";

export const MANIFEST_COLUMNS = ["tracking", "service", "name", "address", "town", "postcode", "country", "weight_kg"];

export function manifestRow(s: Shipment): string[] {
  return [
    s.tracking,
    s.service,
    s.to.name,
    s.to.lines.join(" / "),
    s.to.town,
    s.to.country === "GB" ? normalizePostcode(s.to.postcode) : s.to.postcode,
    s.to.country,
    s.parcel.weightKg.toFixed(1),
  ];
}

export function manifestRows(shipments: Shipment[]): string[][] {
  return [MANIFEST_COLUMNS, ...shipments.map(manifestRow)];
}

export function manifestCsv(shipments: Shipment[]): string {
  return toCsv(manifestRows(shipments));
}
