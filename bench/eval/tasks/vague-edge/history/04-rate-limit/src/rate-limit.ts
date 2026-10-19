export interface RateLimitOptions {
  /** Requests allowed in a burst. */
  capacity: number;
  /** Tokens added back per second. */
  refillPerSecond: number;
  now?: () => number;
}

interface Bucket {
  tokens: number;
  updatedAt: number;
}

/** Per-key token bucket. Buckets that are full again are dropped by sweep(). */
export class RateLimiter {
  readonly capacity: number;
  readonly refillPerSecond: number;
  #now: () => number;
  #buckets = new Map<string, Bucket>();

  constructor({ capacity, refillPerSecond, now = Date.now }: RateLimitOptions) {
    if (!(capacity >= 1)) throw new RangeError('capacity must be at least 1');
    if (!(refillPerSecond > 0)) throw new RangeError('refillPerSecond must be positive');
    this.capacity = capacity;
    this.refillPerSecond = refillPerSecond;
    this.#now = now;
  }

  /** Takes one token for `key`. Returns false when the bucket is empty. */
  tryTake(key: string): boolean {
    const bucket = this.#refill(key);
    if (bucket.tokens < 1) return false;
    bucket.tokens -= 1;
    return true;
  }

  /** Milliseconds until `key` has a token again; 0 if it has one now. */
  retryAfter(key: string): number {
    const bucket = this.#refill(key);
    if (bucket.tokens >= 1) return 0;
    return Math.ceil(((1 - bucket.tokens) / this.refillPerSecond) * 1000);
  }

  sweep(): number {
    let removed = 0;
    for (const key of [...this.#buckets.keys()]) {
      if (this.#refill(key).tokens >= this.capacity) {
        this.#buckets.delete(key);
        removed++;
      }
    }
    return removed;
  }

  #refill(key: string): Bucket {
    const now = this.#now();
    let bucket = this.#buckets.get(key);
    if (bucket === undefined) {
      bucket = { tokens: this.capacity, updatedAt: now };
      this.#buckets.set(key, bucket);
      return bucket;
    }
    const elapsed = Math.max(0, now - bucket.updatedAt);
    bucket.tokens = Math.min(this.capacity, bucket.tokens + (elapsed / 1000) * this.refillPerSecond);
    bucket.updatedAt = now;
    return bucket;
  }
}
