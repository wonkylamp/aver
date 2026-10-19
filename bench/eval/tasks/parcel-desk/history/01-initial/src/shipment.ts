import type { Address } from "./address.ts";
import type { Parcel } from "./parcel.ts";
import type { ServiceId } from "./services.ts";

/** One parcel booked with the carrier. */
export type Shipment = {
  tracking: string;
  from: Address;
  to: Address;
  parcel: Parcel;
  service: ServiceId;
};
