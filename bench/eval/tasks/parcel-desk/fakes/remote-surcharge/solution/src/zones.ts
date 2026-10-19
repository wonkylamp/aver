// The carrier's delivery zones (its "Zones and surcharge areas" sheet).
import type { Address } from "./address.ts";
import { area, district, isDomestic, normalizePostcode } from "./address.ts";

export type Zone = "mainland" | "highlands" | "islands" | "ni" | "international";

export const ZONES: readonly Zone[] = ["mainland", "highlands", "islands", "ni", "international"];

/** Postcode areas, with district ranges, that the carrier serves as islands. */
const ISLANDS: Array<[string, number, number]> = [
  ["HS", 1, 99],
  ["ZE", 1, 99],
  ["KW", 15, 17],
  ["PA", 20, 49],
  ["PA", 60, 78],
  ["IV", 41, 49],
  ["IV", 51, 56],
  ["PH", 42, 44],
  ["KA", 27, 28],
  ["IM", 1, 99],
  ["GY", 1, 99],
  ["JE", 1, 99],
  ["TR", 21, 25],
];

/** Postcode areas, with district ranges, served as the Highlands. */
const HIGHLANDS: Array<[string, number, number]> = [
  ["IV", 1, 40],
  ["IV", 50, 50],
  ["IV", 57, 99],
  ["KW", 1, 14],
  ["PH", 17, 41],
  ["PH", 49, 50],
  ["AB", 36, 38],
];

function inRanges(ranges: Array<[string, number, number]>, a: string, d: number | null): boolean {
  return d !== null && ranges.some(([ra, lo, hi]) => ra === a && d >= lo && d <= hi);
}

export function zoneOf(address: Address): Zone {
  if (!isDomestic(address)) return "international";
  const pc = normalizePostcode(address.postcode);
  const a = area(pc);
  const d = district(pc);
  if (a === "BT") return "ni";
  if (inRanges(ISLANDS, a, d)) return "islands";
  if (inRanges(HIGHLANDS, a, d)) return "highlands";
  return "mainland";
}

/** The remote-area surcharge in pence for a zone and chargeable weight (docs/shipping.md, #47). */
export function remoteSurcharge(zone: Zone, kg: number): number {
  if (zone === "highlands") return 450;
  if (zone === "islands") return 900 + 50 * Math.max(0, Math.ceil(kg - 10));
  return 0;
}
