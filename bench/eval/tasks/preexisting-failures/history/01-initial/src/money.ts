/**
 * Amounts are integers in the currency's minor unit (cents for EUR, GBP and
 * USD, whole yen for JPY). Floats never hold money in this library.
 */
export type Cents = number;

/** A rate in basis points: 2000 is 20%, 550 is 5.5%. */
export type Bps = number;

export function cents(value: number): Cents {
  if (!Number.isSafeInteger(value)) {
    throw new RangeError(`not a whole number of minor units: ${value}`);
  }
  return value;
}

export function sum(values: readonly Cents[]): Cents {
  let total = 0;
  for (const value of values) total += value;
  return total;
}

/**
 * `amount * bps / 10000`, rounded half up to a whole minor unit. Negative
 * amounts round symmetrically (half away from zero).
 */
export function percentOf(amount: Cents, bps: Bps): Cents {
  if (amount < 0) {
    const positive = percentOf(-amount, bps);
    return positive === 0 ? 0 : -positive;
  }
  const scaled = amount * bps + 5000;
  return (scaled - (scaled % 10000)) / 10000;
}

/**
 * Splits `amount` into parts proportional to `weights` so that the parts sum
 * to exactly `amount`.
 *
 * Each part starts as the floor of its exact share. The cents left over are
 * handed out one at a time to the parts with the largest fractional remainder;
 * equal remainders go to the earlier index first.
 */
export function allocate(amount: Cents, weights: readonly number[]): Cents[] {
  if (amount < 0 || !Number.isSafeInteger(amount)) {
    throw new RangeError(`allocate needs a non-negative whole amount, got ${amount}`);
  }
  for (const weight of weights) {
    if (weight < 0 || !Number.isSafeInteger(weight)) {
      throw new RangeError(`allocate needs non-negative whole weights, got ${weight}`);
    }
  }
  const total = sum(weights);
  if (total === 0) {
    if (amount !== 0) throw new RangeError("cannot allocate a non-zero amount over zero weights");
    return weights.map(() => 0);
  }

  const parts = weights.map((weight) => {
    const numerator = amount * weight;
    return (numerator - (numerator % total)) / total;
  });
  let left = amount - sum(parts);
  const order = weights
    .map((weight, index) => ({ index, remainder: (amount * weight) % total }))
    .sort((a, b) => b.remainder - a.remainder || a.index - b.index);
  for (const { index } of order) {
    if (left === 0) break;
    parts[index] += 1;
    left -= 1;
  }
  return parts;
}
