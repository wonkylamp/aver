// Sales tax in basis points (1/100 of a percent) by region.
const RATES_BP: Record<string, number> = {
  "US-CA": 725,
  "US-NY": 400,
  "US-OR": 0,
  "US-WA": 650,
  "EU-DE": 1900,
  "EU-NL": 2100,
};

export function taxRateBp(region: string): number {
  const rate = RATES_BP[region];
  if (rate === undefined) throw new Error(`no tax rate for region ${region}`);
  return rate;
}

export function taxCents(amountCents: number, region: string): number {
  return Math.round((amountCents * taxRateBp(region)) / 10000);
}
