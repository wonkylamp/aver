import type { Day } from "./dates.ts";
import { addBusinessDays } from "./dates.ts";

export type ServiceId = "economy" | "express";

export type Service = {
  name: string;
  /** Business days from hand-over to delivery. */
  transitDays: number;
  /** cm³ per kg for volumetric weight (docs/shipping.md). */
  volumetricDivisor: number;
};

export const SERVICES: Record<ServiceId, Service> = {
  economy: { name: "Economy", transitDays: 3, volumetricDivisor: 6000 },
  express: { name: "Express", transitDays: 1, volumetricDivisor: 5000 },
};

export function serviceOf(id: string): Service {
  const s = SERVICES[id as ServiceId];
  if (!s) throw new Error(`no service ${id}`);
  return s;
}

/** The day a parcel handed over on `day` should arrive. */
export function estimateDelivery(day: Day, service: ServiceId): Day {
  return addBusinessDays(day, SERVICES[service].transitDays);
}
