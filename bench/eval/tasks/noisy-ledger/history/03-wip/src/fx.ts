// Currency conversion with daily reference rates (ECB, EUR base).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { addDays } from "./dates.ts";
import { minorUnits } from "./money.ts";

/** date -> currency -> units of that currency per 1 EUR. */
export type RateTable = Record<string, Record<string, number>>;

export class FxError extends Error {
  override name = "FxError";
}

export const RATES_FILE = join(import.meta.dirname, "..", "fixtures", "rates-2025.json");

/** The 2025 reference rates, fetched by `npm run fetch-rates` (not committed: the licence forbids it). */
export function loadRates(file = RATES_FILE): RateTable {
  return JSON.parse(readFileSync(file, "utf8")) as RateTable;
}

/** The rate on `date`, or on the latest earlier day within a week (weekends, holidays). */
export function rateOn(table: RateTable, currency: string, date: string): number {
  if (currency === "EUR") return 1;
  for (let i = 0; i < 7; i++) {
    const r = table[addDays(date, -i)]?.[currency];
    if (r !== undefined) return r;
  }
  throw new FxError(`no ${currency} rate within a week of ${date}`);
}

/** Converts minor units of `from` into minor units of `to`, rounding half away from zero. */
export function convert(table: RateTable, minor: number, from: string, to: string, date: string): number {
  if (from === to) return minor;
  const eur = minor / 10 ** minorUnits(from) / rateOn(table, from, date);
  const value = eur * rateOn(table, to, date) * 10 ** minorUnits(to);
  return Math.sign(value) * Math.round(Math.abs(value));
}
