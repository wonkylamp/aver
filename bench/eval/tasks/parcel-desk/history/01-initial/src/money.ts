// Amounts are integer pence.

/** Rounds half away from zero (2.5 -> 3, -2.5 -> -3). */
export function roundHalfUp(x: number): number {
  return x < 0 ? -Math.round(-x) : Math.round(x);
}

/** `£12.34`, `-£0.05`. */
export function formatPence(pence: number): string {
  const sign = pence < 0 ? "-" : "";
  const abs = Math.abs(pence);
  return `${sign}£${Math.floor(abs / 100)}.${String(abs % 100).padStart(2, "0")}`;
}

export function sum(xs: number[]): number {
  return xs.reduce((a, b) => a + b, 0);
}

/** `rate` percent of `pence`, rounded half up. */
export function percentOf(pence: number, rate: number): number {
  return roundHalfUp((pence * rate) / 100);
}
