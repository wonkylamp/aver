export class TimeoutError extends Error {
  readonly ms: number;

  constructor(ms: number, message = `timed out after ${ms}ms`) {
    super(message);
    this.name = 'TimeoutError';
    this.ms = ms;
  }
}

/** Thrown by retry when every attempt failed. `cause` is the last error. */
export class RetryError extends Error {
  readonly attempts: number;

  constructor(attempts: number, cause: unknown) {
    super(`gave up after ${attempts} attempt${attempts === 1 ? '' : 's'}`, { cause });
    this.name = 'RetryError';
    this.attempts = attempts;
  }
}

export class QueueClearedError extends Error {
  constructor() {
    super('task was removed from the queue before it started');
    this.name = 'QueueClearedError';
  }
}
