import type { ManualClock } from '../clock.ts';

/** Lets every queued promise callback run. */
export function settle(): Promise<void> {
  return new Promise<void>((resolve) => setImmediate(resolve));
}

/**
 * Advances a ManualClock by `ms`, one timer at a time, letting promise
 * callbacks run after each timer. Use it for promise-based code such as
 * retry and sleep, where a timer's effect only lands after a microtask.
 */
export async function advanceAsync(clock: ManualClock, ms: number): Promise<void> {
  const target = clock.now() + ms;
  await settle();
  for (;;) {
    const due = clock.nextDue();
    if (due === undefined || due > target) break;
    clock.advanceTo(due);
    await settle();
  }
  clock.advanceTo(target);
  await settle();
}
