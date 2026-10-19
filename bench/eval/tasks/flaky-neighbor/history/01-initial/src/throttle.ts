import { systemClock, type Clock, type TimerHandle } from './clock.ts';
import { assertDuration } from './duration.ts';

export interface ThrottleOptions {
  /** Invoke on the first call of a window. Default true. */
  leading?: boolean;
  /** Invoke once more at the end of a window if calls arrived during it. Default true. */
  trailing?: boolean;
  clock?: Clock;
}

export interface Throttled<A extends unknown[]> {
  (...args: A): void;
  cancel(): void;
}

/**
 * Runs `fn` at most once per `interval` ms. Calls made during a window are
 * collapsed into one trailing call with the latest arguments.
 */
export function throttle<A extends unknown[]>(
  fn: (...args: A) => void,
  interval: number,
  options: ThrottleOptions = {},
): Throttled<A> {
  assertDuration('interval', interval);
  const clock = options.clock ?? systemClock;
  const leading = options.leading ?? true;
  const trailing = options.trailing ?? true;

  let timer: TimerHandle | undefined;
  let pendingArgs: A | undefined;

  function windowEnded(): void {
    timer = undefined;
    if (trailing && pendingArgs !== undefined) {
      const args = pendingArgs;
      pendingArgs = undefined;
      fn(...args);
      // The trailing call opens a new window so calls stay spaced out.
      timer = clock.setTimeout(windowEnded, interval);
    }
  }

  const throttled = ((...args: A): void => {
    if (timer === undefined) {
      timer = clock.setTimeout(windowEnded, interval);
      if (leading) {
        fn(...args);
        return;
      }
    }
    pendingArgs = args;
  }) as Throttled<A>;

  throttled.cancel = (): void => {
    if (timer !== undefined) clock.clearTimeout(timer);
    timer = undefined;
    pendingArgs = undefined;
  };

  return throttled;
}
