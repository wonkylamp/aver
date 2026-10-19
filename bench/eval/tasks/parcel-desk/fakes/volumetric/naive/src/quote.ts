// The price of sending a parcel: the band price for its weight, the
// surcharges, and VAT on both (docs/shipping.md, "Prices").
import type { Address } from "./address.ts";
import { checkAddress } from "./address.ts";
import type { Day } from "./dates.ts";
import { percentOf, sum } from "./money.ts";
import type { Parcel } from "./parcel.ts";
import { chargeableKg, checkParcel } from "./parcel.ts";
import { bandPrice } from "./rates.ts";
import type { ServiceId } from "./services.ts";
import { estimateDelivery } from "./services.ts";
import type { Zone } from "./zones.ts";
import { zoneOf } from "./zones.ts";

export type Surcharge = { code: string; pence: number };

export type Quote = {
  service: ServiceId;
  zone: Zone;
  /** The weight the price is for. */
  chargeableKg: number;
  basePence: number;
  /** In the order docs/shipping.md lists them. */
  surcharges: Surcharge[];
  /** Base price plus surcharges. */
  netPence: number;
  vatPence: number;
  totalPence: number;
  deliveryBy: Day;
};

export const FUEL_PERCENT = 3;
export const VAT_PERCENT = 20;

export function quote(parcel: Parcel, to: Address, service: ServiceId, day: Day): Quote {
  checkParcel(parcel);
  checkAddress(to);
  const zone = zoneOf(to);
  const kg = chargeableKg(parcel, service);
  const basePence = bandPrice(service, zone, kg);
  const surcharges: Surcharge[] = [{ code: "FUEL", pence: percentOf(basePence, FUEL_PERCENT) }];
  const netPence = basePence + sum(surcharges.map((s) => s.pence));
  const vatPence = percentOf(netPence, VAT_PERCENT);
  return {
    service,
    zone,
    chargeableKg: kg,
    basePence,
    surcharges,
    netPence,
    vatPence,
    totalPence: netPence + vatPence,
    deliveryBy: estimateDelivery(day, service),
  };
}
