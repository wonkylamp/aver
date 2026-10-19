import { test } from 'node:test';
import assert from 'node:assert/strict';
import { constant, exponential } from '../src/backoff.ts';
import { ManualClock } from '../src/clock.ts';
import { RetryError } from '../src/errors.ts';
import { retry } from '../src/retry.ts';
import { advanceAsync, settle } from '../src/testing/settle.ts';

function failTimes(n: number, clock: ManualClock) {
  const attempts: number[] = [];
  const op = async (attempt: number): Promise<string> => {
    attempts.push(clock.now());
    if (attempt <= n) throw new Error(`fail ${attempt}`);
    return 'ok';
  };
  return { attempts, op };
}

test('returns the first successful result without waiting', async () => {
  const clock = new ManualClock();
  const { attempts, op } = failTimes(0, clock);
  assert.equal(await retry(op, { clock }), 'ok');
  assert.deepEqual(attempts, [0]);
});

test('waits the backoff delay between attempts', async () => {
  const clock = new ManualClock();
  const { attempts, op } = failTimes(2, clock);
  const result = retry(op, { clock, backoff: exponential({ initial: 100 }) });
  await advanceAsync(clock, 1_000);
  assert.equal(await result, 'ok');
  assert.deepEqual(attempts, [0, 100, 300]);
});

test('gives up after the configured retries with the last error as cause', async () => {
  const clock = new ManualClock();
  const { attempts, op } = failTimes(10, clock);
  const result = retry(op, { clock, retries: 2, backoff: constant(10) });
  const assertion = assert.rejects(result, (error: unknown) => {
    assert.ok(error instanceof RetryError);
    assert.equal(error.attempts, 3);
    assert.match(String((error.cause as Error).message), /fail 3/);
    return true;
  });
  await advanceAsync(clock, 100);
  await assertion;
  assert.equal(attempts.length, 3);
});

test('shouldRetry can stop early and rethrow the original error', async () => {
  const clock = new ManualClock();
  const result = retry(
    async () => {
      throw new TypeError('not retryable');
    },
    { clock, shouldRetry: (error) => !(error instanceof TypeError) },
  );
  await assert.rejects(result, TypeError);
});

test('onRetry sees each failure and the delay before the next attempt', async () => {
  const clock = new ManualClock();
  const { op } = failTimes(2, clock);
  const seen: [number, number][] = [];
  const result = retry(op, {
    clock,
    backoff: constant(25),
    onRetry: (_error, attempt, delay) => seen.push([attempt, delay]),
  });
  await advanceAsync(clock, 100);
  await result;
  assert.deepEqual(seen, [
    [1, 25],
    [2, 25],
  ]);
});

test('an abort during the backoff rejects with the abort reason', async () => {
  const clock = new ManualClock();
  const controller = new AbortController();
  const { op } = failTimes(5, clock);
  const result = retry(op, { clock, backoff: constant(1_000), signal: controller.signal });
  const assertion = assert.rejects(result, /shutting down/);
  await settle();
  controller.abort(new Error('shutting down'));
  await assertion;
});

test('rejects a negative retry count', async () => {
  await assert.rejects(retry(async () => 1, { retries: -1 }), RangeError);
});
