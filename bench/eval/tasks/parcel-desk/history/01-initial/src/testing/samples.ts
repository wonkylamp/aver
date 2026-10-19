// Addresses and parcels the tests share.
import type { Address } from "../address.ts";
import type { Parcel } from "../parcel.ts";
import type { Shipment } from "../shipment.ts";

export const WAREHOUSE: Address = { name: "Dispatch Ltd", lines: ["Unit 4, Ring Road"], town: "Leeds", postcode: "LS11 5QX", country: "GB" };

export const LONDON: Address = { name: "Ada Lovelace", lines: ["12 Marsham Street"], town: "London", postcode: "SW1P 4DF", country: "GB" };

export const BELFAST: Address = { name: "Joe Doyle", lines: ["3 Linen Hall Street"], town: "Belfast", postcode: "BT1 1AA", country: "GB" };

export const INVERNESS: Address = { name: "Morag Ross", lines: ["8 Castle Road"], town: "Inverness", postcode: "IV2 3EA", country: "GB" };

export const KIRKWALL: Address = { name: "Erik Flett", lines: ["1 Broad Street"], town: "Kirkwall", postcode: "KW15 1DH", country: "GB" };

export const PARIS: Address = { name: "Marie Curie", lines: ["11 rue Pierre et Marie Curie"], town: "Paris", postcode: "75005", country: "FR" };

/** 2.5 kg in a 30 × 20 × 10 cm box. */
export const BOX: Parcel = { weightKg: 2.5, lengthCm: 30, widthCm: 20, heightCm: 10 };

/** 0.8 kg in a 20 × 15 × 10 cm box. */
export const SMALL: Parcel = { weightKg: 0.8, lengthCm: 20, widthCm: 15, heightCm: 10 };

export function shipment(over: Partial<Shipment> = {}): Shipment {
  return { tracking: "JD000001244GB", from: WAREHOUSE, to: LONDON, parcel: BOX, service: "economy", ...over };
}
