import { exponential, type Backoff } from './backoff.ts';
import { systemClock, type Clock } from './clock.ts';
import { RetryError } from './errors.ts';
import { sleep } from './sleep.ts';

export interface RetryOptions {
  /** Retries after the first attempt. Default 3, so at most 4 attempts. */
  retries?: number;
  backoff?: Backoff;
  /** Return false to stop retrying and rethrow the error as is. */
  shouldRetry?: (error: unknown, attempt: number) => boolean;
  onRetry?: (error: unknown, attempt: number, delay: number) => void;
  clock?: Clock;
  signal?: AbortSignal;
}

const defaultBackoff = exponential({ initial: 100, max: 5_000 });

/**
 * Runs `operation` until it resolves or the retries run out. `attempt` is
 * 1 for the first call. Throws RetryError with the last error as `cause`.
 */
export async function retry<T>(
  operation: (attempt: number) => Promise<T>,
  options: RetryOptions = {},
): Promise<T> {
  const retries = options.retries ?? 3;
  if (!Number.isInteger(retries) || retries < 0) {
    throw new RangeError(`retries must be a non-negative integer, got ${retries}`);
  }
  const backoff = options.backoff ?? defaultBackoff;
  const clock = options.clock ?? systemClock;

  for (let attempt = 1; ; attempt++) {
    options.signal?.throwIfAborted();
    try {
      return await operation(attempt);
    } catch (error) {
      if (options.shouldRetry && !options.shouldRetry(error, attempt)) throw error;
      if (attempt > retries) throw new RetryError(attempt, error);
      const delay = backoff(attempt);
      options.onRetry?.(error, attempt, delay);
      await sleep(delay, clock, options.signal);
    }
  }
}
