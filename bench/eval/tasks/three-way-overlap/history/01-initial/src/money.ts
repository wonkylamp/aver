// Integer minor units everywhere, and the two rounding rules we use.

/** Half away from zero, to a whole number of minor units. */
export function roundHalfUp(x: number): number {
  return Math.sign(x) * Math.round(Math.abs(x) + 1e-9);
}

/**
 * Splits `total` over `weights` in proportion, in whole units, so the parts
 * add up to `total` exactly: largest remainder, ties to the earlier weight.
 */
export function allocate(total: number, weights: number[]): number[] {
  const sum = weights.reduce((a, b) => a + b, 0);
  if (sum === 0) return weights.map(() => 0);
  const exact = weights.map((w) => (total * w) / sum);
  const parts = exact.map((x) => Math.floor(x));
  let left = total - parts.reduce((a, b) => a + b, 0);
  const order = exact.map((x, i) => [x - Math.floor(x), i] as const).sort((a, b) => b[0] - a[0] || a[1] - b[1]);
  for (const [, i] of order) {
    if (left <= 0) break;
    parts[i]! += 1;
    left -= 1;
  }
  return parts;
}

export function format(minor: number, currency: string): string {
  const digits = currency === "JPY" ? 0 : 2;
  const abs = Math.abs(minor);
  const s = digits ? `${Math.floor(abs / 100)}.${String(abs % 100).padStart(2, "0")}` : String(abs);
  return `${minor < 0 ? "-" : ""}${currency} ${s}`;
}
