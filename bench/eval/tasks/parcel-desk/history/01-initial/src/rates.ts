// The carrier's 2026 rate card: the price of a parcel by weight band, per
// service and zone, before surcharges and VAT.
import type { ServiceId } from "./services.ts";
import type { Zone } from "./zones.ts";

export type Band = { upToKg: number; pence: number };

export class RateError extends Error {}

const BANDS = [1, 2, 5, 10, 20, 30];

function card(prices: number[]): Band[] {
  return BANDS.map((upToKg, i) => ({ upToKg, pence: prices[i]! }));
}

export const RATE_CARD: Record<ServiceId, Record<Zone, Band[]>> = {
  economy: {
    mainland: card([349, 449, 649, 899, 1399, 1899]),
    highlands: card([449, 599, 849, 1199, 1799, 2499]),
    islands: card([549, 699, 999, 1399, 2099, 2899]),
    ni: card([499, 649, 899, 1249, 1899, 2599]),
    international: card([1199, 1599, 2299, 3299, 4999, 6999]),
  },
  express: {
    mainland: card([699, 849, 1099, 1499, 2199, 2899]),
    highlands: card([899, 1099, 1399, 1899, 2799, 3699]),
    islands: card([1099, 1299, 1699, 2299, 3299, 4299]),
    ni: card([999, 1199, 1499, 1999, 2899, 3799]),
    international: card([2199, 2799, 3699, 4999, 7499, 9999]),
  },
};

/** The band price for a chargeable weight: the first band whose limit is at least `kg`. */
export function bandPrice(service: ServiceId, zone: Zone, kg: number): number {
  const band = RATE_CARD[service][zone].find((b) => kg <= b.upToKg);
  if (!band) throw new RateError(`${kg} kg is over the ${service} limit`);
  return band.pence;
}
