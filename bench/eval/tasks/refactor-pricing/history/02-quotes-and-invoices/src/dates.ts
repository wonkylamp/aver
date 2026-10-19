const ISO_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

// Calendar arithmetic on ISO dates in UTC, so the result never depends on the
// machine's time zone.
export function addDays(isoDate: string, days: number): string {
  const match = ISO_DATE.exec(isoDate);
  if (match === null) throw new Error(`not an ISO date: ${isoDate}`);
  const time = Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  return new Date(time + days * 86_400_000).toISOString().slice(0, 10);
}
