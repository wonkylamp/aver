import type { RoundingPolicy } from "./rounding.ts";

export type Role = "engineer" | "designer" | "manager";

/** One stretch of tracked time. */
export interface Entry {
  id: string;
  person: string;
  client: string;
  role: Role;
  start: Date;
  /** Tracked time in whole seconds (timers record to the second). */
  seconds: number;
  billable: boolean;
  note?: string;
}

export interface Client {
  id: string;
  name: string;
  /** VAT in percent, e.g. 20. */
  vatRate: number;
  /** Cents per hour by role. */
  rates: Record<Role, number>;
  policy?: RoundingPolicy;
  /** Whether this client pays the overtime premium. */
  overtime: boolean;
}
