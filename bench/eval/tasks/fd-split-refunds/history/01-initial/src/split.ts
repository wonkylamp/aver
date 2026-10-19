// Splitting an amount of pence among people.

export class SplitError extends Error {}

/** `total` split among `n` people. */
export function splitEvenly(total: number, n: number): number[] {
  if (!Number.isInteger(n) || n < 1) throw new SplitError(`cannot split among ${n}`);
  if (!Number.isInteger(total)) throw new SplitError(`not a whole number of pence: ${total}`);
  return Array.from({ length: n }, () => Math.floor(total / n));
}

/**
 * `total` split in proportion to `shares` (largest remainder: the parts add
 * up to the total; leftover pence go to the largest remainders, then the
 * earliest). A negative total splits as the negation of its positive.
 */
export function splitByShares(total: number, shares: number[]): number[] {
  if (shares.length === 0 || shares.some((s) => !(s >= 0)) || shares.every((s) => s === 0)) throw new SplitError("shares must be non-negative and not all zero");
  if (!Number.isInteger(total)) throw new SplitError(`not a whole number of pence: ${total}`);
  if (total < 0) return splitByShares(-total, shares).map((p) => (p === 0 ? 0 : -p));
  const weight = shares.reduce((a, b) => a + b, 0);
  const exact = shares.map((s) => (total * s) / weight);
  const parts = exact.map(Math.floor);
  let left = total - parts.reduce((a, b) => a + b, 0);
  const order = exact.map((x, i) => ({ i, r: x - Math.floor(x) })).sort((a, b) => b.r - a.r || a.i - b.i);
  for (const { i } of order) {
    if (left === 0) break;
    parts[i]! += 1;
    left -= 1;
  }
  return parts;
}
