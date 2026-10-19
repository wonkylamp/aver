import { assertDuration } from './duration.ts';

/** Maps a 1-based retry number to a delay in milliseconds. */
export type Backoff = (retry: number) => number;

export function constant(ms: number): Backoff {
  assertDuration('ms', ms);
  return () => ms;
}

export function linear(initial: number, step: number, max = Number.MAX_SAFE_INTEGER): Backoff {
  assertDuration('initial', initial);
  assertDuration('step', step);
  return (retry) => Math.min(max, initial + step * (retry - 1));
}

export interface ExponentialOptions {
  initial: number;
  factor?: number;
  max?: number;
}

export function exponential({ initial, factor = 2, max = Number.MAX_SAFE_INTEGER }: ExponentialOptions): Backoff {
  assertDuration('initial', initial);
  if (!(factor >= 1)) throw new RangeError(`factor must be at least 1, got ${factor}`);
  return (retry) => Math.min(max, initial * factor ** (retry - 1));
}

/**
 * "Full jitter": a uniformly random delay between 0 and the wrapped delay.
 * `random` is injectable so tests stay deterministic.
 */
export function withFullJitter(backoff: Backoff, random: () => number = Math.random): Backoff {
  return (retry) => Math.floor(random() * backoff(retry));
}
