import { systemClock, type Clock, type TimerHandle } from './clock.ts';
import { assertDuration } from './duration.ts';

export interface DebounceOptions {
  /** Invoke `fn` on the first call of a burst. Default false. */
  leading?: boolean;
  /** Invoke `fn` once the burst has been quiet for `wait` ms. Default true. */
  trailing?: boolean;
  /** Clock used for timers. Defaults to the system clock. */
  clock?: Clock;
}

export interface Debounced<A extends unknown[]> {
  (...args: A): void;
  /** Ends the current burst and drops the pending call, if any. */
  cancel(): void;
  /** Ends the current burst now, as if `wait` had elapsed. */
  flush(): void;
  /** True while a burst is in progress. */
  pending(): boolean;
}

/**
 * Coalesces a burst of calls into one call of `fn`.
 *
 * A burst starts with a call made while no burst is in progress and lasts
 * until `wait` ms pass without a call. Each call restarts the wait. When the
 * burst ends, `fn` runs once with the arguments of the latest call (the
 * trailing call), unless that call was already consumed by the leading call.
 */
export function debounce<A extends unknown[]>(
  fn: (...args: A) => void,
  wait: number,
  options: DebounceOptions = {},
): Debounced<A> {
  assertDuration('wait', wait);
  const clock = options.clock ?? systemClock;
  const leading = options.leading ?? false;
  const trailing = options.trailing ?? true;

  let timer: TimerHandle | undefined;
  // Arguments of the latest call that has not been passed to fn yet.
  let pendingArgs: A | undefined;

  function invoke(): void {
    const args = pendingArgs as A;
    pendingArgs = undefined;
    fn(...args);
  }

  function endBurst(): void {
    timer = undefined;
    if (trailing && pendingArgs !== undefined) invoke();
    pendingArgs = undefined;
  }

  const debounced = ((...args: A): void => {
    pendingArgs = args;
    const startsBurst = timer === undefined;
    if (timer !== undefined) clock.clearTimeout(timer);
    timer = clock.setTimeout(endBurst, wait);
    if (startsBurst && leading) invoke();
  }) as Debounced<A>;

  debounced.cancel = (): void => {
    if (timer !== undefined) clock.clearTimeout(timer);
    timer = undefined;
    pendingArgs = undefined;
  };

  debounced.flush = (): void => {
    if (timer === undefined) return;
    clock.clearTimeout(timer);
    endBurst();
  };

  debounced.pending = (): boolean => timer !== undefined;

  return debounced;
}
