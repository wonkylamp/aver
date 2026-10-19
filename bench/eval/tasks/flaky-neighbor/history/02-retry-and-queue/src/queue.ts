import { createDeferred, type Deferred } from './deferred.ts';
import { QueueClearedError } from './errors.ts';

export interface TaskQueueOptions {
  /** Tasks allowed to run at once. Default 1. */
  concurrency?: number;
  /** Start paused; call resume() to begin. Default false. */
  paused?: boolean;
}

interface Entry {
  run: () => Promise<unknown>;
  done: Deferred<unknown>;
}

/** Runs async tasks in FIFO order with a concurrency limit. */
export class TaskQueue {
  readonly concurrency: number;
  #waiting: Entry[] = [];
  #running = 0;
  #paused: boolean;
  #idle: Deferred<void>[] = [];

  constructor(options: TaskQueueOptions = {}) {
    const concurrency = options.concurrency ?? 1;
    if (!Number.isInteger(concurrency) || concurrency < 1) {
      throw new RangeError(`concurrency must be a positive integer, got ${concurrency}`);
    }
    this.concurrency = concurrency;
    this.#paused = options.paused ?? false;
  }

  /** Tasks waiting to start. */
  get size(): number {
    return this.#waiting.length;
  }

  /** Tasks currently running. */
  get running(): number {
    return this.#running;
  }

  get paused(): boolean {
    return this.#paused;
  }

  add<T>(task: () => Promise<T>): Promise<T> {
    const done = createDeferred<unknown>();
    this.#waiting.push({ run: task, done });
    this.#drain();
    return done.promise as Promise<T>;
  }

  pause(): void {
    this.#paused = true;
  }

  resume(): void {
    this.#paused = false;
    this.#drain();
  }

  /** Rejects every task that has not started with QueueClearedError. */
  clear(): void {
    const waiting = this.#waiting;
    this.#waiting = [];
    for (const entry of waiting) entry.done.reject(new QueueClearedError());
    this.#settleIdle();
  }

  /** Resolves once nothing is running and nothing is waiting. */
  onIdle(): Promise<void> {
    if (this.#running === 0 && this.#waiting.length === 0) return Promise.resolve();
    const idle = createDeferred<void>();
    this.#idle.push(idle);
    return idle.promise;
  }

  #drain(): void {
    while (!this.#paused && this.#running < this.concurrency && this.#waiting.length > 0) {
      const entry = this.#waiting.shift() as Entry;
      this.#running++;
      void this.#start(entry);
    }
  }

  async #start(entry: Entry): Promise<void> {
    try {
      entry.done.resolve(await entry.run());
    } catch (error) {
      entry.done.reject(error);
    } finally {
      this.#running--;
      this.#drain();
      this.#settleIdle();
    }
  }

  #settleIdle(): void {
    if (this.#running !== 0 || this.#waiting.length !== 0) return;
    const idle = this.#idle;
    this.#idle = [];
    for (const d of idle) d.resolve();
  }
}
