/**
 * Every utility in this package reads time and schedules timers through a
 * Clock, so tests can swap in ManualClock and control time exactly.
 */

export type TimerHandle = number;

export interface Clock {
  /** Milliseconds since an arbitrary origin. Only differences are meaningful. */
  now(): number;
  setTimeout(callback: () => void, ms: number): TimerHandle;
  clearTimeout(handle: TimerHandle): void;
}

export class SystemClock implements Clock {
  #nextId = 0;
  #timers = new Map<TimerHandle, ReturnType<typeof setTimeout>>();

  now(): number {
    return Date.now();
  }

  setTimeout(callback: () => void, ms: number): TimerHandle {
    const id = ++this.#nextId;
    const timer = setTimeout(() => {
      this.#timers.delete(id);
      callback();
    }, ms);
    this.#timers.set(id, timer);
    return id;
  }

  clearTimeout(handle: TimerHandle): void {
    const timer = this.#timers.get(handle);
    if (timer !== undefined) {
      clearTimeout(timer);
      this.#timers.delete(handle);
    }
  }
}

export const systemClock: Clock = new SystemClock();

interface ManualTimer {
  due: number;
  callback: () => void;
}

/**
 * A clock that only moves when told to. Timers fire in order of due time;
 * timers due at the same instant fire in the order they were scheduled.
 */
export class ManualClock implements Clock {
  #now: number;
  #nextId = 0;
  #timers = new Map<TimerHandle, ManualTimer>();

  constructor(start = 0) {
    this.#now = start;
  }

  now(): number {
    return this.#now;
  }

  setTimeout(callback: () => void, ms: number): TimerHandle {
    const id = ++this.#nextId;
    this.#timers.set(id, { due: this.#now + Math.max(0, ms), callback });
    return id;
  }

  clearTimeout(handle: TimerHandle): void {
    this.#timers.delete(handle);
  }

  /** Number of scheduled timers that have not fired or been cleared. */
  pendingTimers(): number {
    return this.#timers.size;
  }

  /** Due time of the earliest scheduled timer, if any. */
  nextDue(): number | undefined {
    return this.#earliest()?.[1].due;
  }

  /**
   * Moves time forward by `ms`, firing every timer that falls due on the way,
   * including timers scheduled by callbacks that fire during the advance.
   */
  advance(ms: number): void {
    if (!(ms >= 0) || !Number.isFinite(ms)) {
      throw new RangeError(`cannot advance a ManualClock by ${ms}`);
    }
    const target = this.#now + ms;
    for (;;) {
      const next = this.#earliest();
      if (next === undefined || next[1].due > target) break;
      const [id, timer] = next;
      this.#timers.delete(id);
      this.#now = timer.due;
      timer.callback();
    }
    this.#now = target;
  }

  /** Advances to the moment `time`. Time never moves backwards. */
  advanceTo(time: number): void {
    if (time < this.#now) {
      throw new RangeError(`cannot move a ManualClock back from ${this.#now} to ${time}`);
    }
    this.advance(time - this.#now);
  }

  /** Fires timers until none are left. Throws if timers keep rescheduling. */
  runAll(limit = 10_000): void {
    for (let fired = 0; fired < limit; fired++) {
      const due = this.nextDue();
      if (due === undefined) return;
      this.advanceTo(due);
    }
    throw new Error(`ManualClock.runAll: still busy after ${limit} timers`);
  }

  #earliest(): [TimerHandle, ManualTimer] | undefined {
    let best: [TimerHandle, ManualTimer] | undefined;
    for (const entry of this.#timers) {
      // Map iteration follows insertion order, so ids break ties by schedule order.
      if (best === undefined || entry[1].due < best[1].due) best = entry;
    }
    return best;
  }
}
