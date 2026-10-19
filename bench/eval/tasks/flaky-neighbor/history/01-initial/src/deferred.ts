export interface Deferred<T> {
  readonly promise: Promise<T>;
  resolve(value: T): void;
  reject(reason: unknown): void;
  /** True once resolve or reject has been called. */
  readonly settled: boolean;
}

export function createDeferred<T>(): Deferred<T> {
  let settled = false;
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return {
    promise,
    resolve(value: T) {
      if (settled) return;
      settled = true;
      resolve(value);
    },
    reject(reason: unknown) {
      if (settled) return;
      settled = true;
      reject(reason);
    },
    get settled() {
      return settled;
    },
  };
}
