import { systemClock, type Clock } from './clock.ts';
import { assertDuration } from './duration.ts';

export interface TtlCacheOptions {
  /** Default lifetime of an entry in ms. */
  ttl: number;
  /** Evict the least recently used entry beyond this many. Default unlimited. */
  maxEntries?: number;
  clock?: Clock;
}

interface Entry<V> {
  value: V;
  expiresAt: number;
}

/**
 * A map whose entries expire. Expired entries are dropped lazily on access
 * and in bulk by prune(); no timers are kept, so an idle cache costs nothing.
 */
export class TtlCache<K, V> {
  readonly ttl: number;
  readonly maxEntries: number;
  #clock: Clock;
  // Map order doubles as recency order: reads move an entry to the end.
  #entries = new Map<K, Entry<V>>();

  constructor(options: TtlCacheOptions) {
    this.ttl = assertDuration('ttl', options.ttl);
    this.maxEntries = options.maxEntries ?? Number.POSITIVE_INFINITY;
    if (!(this.maxEntries >= 1)) {
      throw new RangeError(`maxEntries must be at least 1, got ${this.maxEntries}`);
    }
    this.#clock = options.clock ?? systemClock;
  }

  get size(): number {
    return this.#entries.size;
  }

  get(key: K): V | undefined {
    const entry = this.#live(key);
    if (entry === undefined) return undefined;
    this.#entries.delete(key);
    this.#entries.set(key, entry);
    return entry.value;
  }

  has(key: K): boolean {
    return this.#live(key) !== undefined;
  }

  set(key: K, value: V, ttl = this.ttl): this {
    assertDuration('ttl', ttl);
    this.#entries.delete(key);
    this.#entries.set(key, { value, expiresAt: this.#clock.now() + ttl });
    while (this.#entries.size > this.maxEntries) {
      const oldest = this.#entries.keys().next().value as K;
      this.#entries.delete(oldest);
    }
    return this;
  }

  delete(key: K): boolean {
    return this.#entries.delete(key);
  }

  clear(): void {
    this.#entries.clear();
  }

  /** Removes every expired entry and returns how many were removed. */
  prune(): number {
    const now = this.#clock.now();
    let removed = 0;
    for (const [key, entry] of this.#entries) {
      if (entry.expiresAt <= now) {
        this.#entries.delete(key);
        removed++;
      }
    }
    return removed;
  }

  /** Returns the cached value, or computes, stores and returns it. */
  async getOrCompute(key: K, compute: () => Promise<V>, ttl = this.ttl): Promise<V> {
    const hit = this.#live(key);
    if (hit !== undefined) return hit.value;
    const value = await compute();
    this.set(key, value, ttl);
    return value;
  }

  #live(key: K): Entry<V> | undefined {
    const entry = this.#entries.get(key);
    if (entry === undefined) return undefined;
    if (entry.expiresAt <= this.#clock.now()) {
      this.#entries.delete(key);
      return undefined;
    }
    return entry;
  }
}
