import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ManualClock } from '../src/clock.ts';
import { createDeferred } from '../src/deferred.ts';
import { TimeoutError } from '../src/errors.ts';
import { withTimeout } from '../src/timeout.ts';
import { settle } from '../src/testing/settle.ts';

test('resolves with the value when the promise wins', async () => {
  const clock = new ManualClock();
  const d = createDeferred<string>();
  const p = withTimeout(d.promise, 100, clock);
  d.resolve('fast');
  assert.equal(await p, 'fast');
  await settle();
  assert.equal(clock.pendingTimers(), 0);
});

test('rejects with TimeoutError when the clock wins', async () => {
  const clock = new ManualClock();
  const d = createDeferred<string>();
  const p = withTimeout(d.promise, 100, clock);
  clock.advance(100);
  await assert.rejects(p, (error: unknown) => error instanceof TimeoutError && error.ms === 100);
});

test('passes through the original rejection', async () => {
  const clock = new ManualClock();
  const d = createDeferred<string>();
  const p = withTimeout(d.promise, 100, clock);
  d.reject(new Error('upstream'));
  await assert.rejects(p, /upstream/);
});
