import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ManualClock } from '../src/clock.ts';
import { sleep } from '../src/sleep.ts';

test('resolves once the clock reaches the deadline', async () => {
  const clock = new ManualClock();
  let done = false;
  const p = sleep(100, clock).then(() => {
    done = true;
  });
  clock.advance(99);
  await Promise.resolve();
  assert.equal(done, false);
  clock.advance(1);
  await p;
  assert.equal(done, true);
});

test('rejects when the signal aborts and clears its timer', async () => {
  const clock = new ManualClock();
  const controller = new AbortController();
  const p = sleep(100, clock, controller.signal);
  controller.abort(new Error('stop'));
  await assert.rejects(p, /stop/);
  assert.equal(clock.pendingTimers(), 0);
});

test('rejects immediately for an already aborted signal', async () => {
  const clock = new ManualClock();
  await assert.rejects(sleep(10, clock, AbortSignal.abort(new Error('already'))), /already/);
  assert.equal(clock.pendingTimers(), 0);
});

test('rejects invalid durations', () => {
  assert.throws(() => sleep(-5), RangeError);
});
