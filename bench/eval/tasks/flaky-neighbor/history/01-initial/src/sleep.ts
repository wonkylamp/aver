import { systemClock, type Clock } from './clock.ts';
import { assertDuration } from './duration.ts';

/** Resolves after `ms` milliseconds on `clock`. Rejects early if `signal` aborts. */
export function sleep(ms: number, clock: Clock = systemClock, signal?: AbortSignal): Promise<void> {
  assertDuration('ms', ms);
  if (signal?.aborted) return Promise.reject(signal.reason);
  return new Promise<void>((resolve, reject) => {
    const onAbort = (): void => {
      clock.clearTimeout(handle);
      reject(signal?.reason);
    };
    const handle = clock.setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}
