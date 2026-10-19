import { systemClock, type Clock } from './clock.ts';
import { assertDuration } from './duration.ts';
import { TimeoutError } from './errors.ts';

/** Settles like `promise`, or rejects with TimeoutError after `ms`. */
export function withTimeout<T>(promise: Promise<T>, ms: number, clock: Clock = systemClock): Promise<T> {
  assertDuration('ms', ms);
  return new Promise<T>((resolve, reject) => {
    const handle = clock.setTimeout(() => reject(new TimeoutError(ms)), ms);
    promise.then(
      (value) => {
        clock.clearTimeout(handle);
        resolve(value);
      },
      (error: unknown) => {
        clock.clearTimeout(handle);
        reject(error);
      },
    );
  });
}
