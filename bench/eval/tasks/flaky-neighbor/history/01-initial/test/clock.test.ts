import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ManualClock, SystemClock } from '../src/clock.ts';

test('ManualClock starts at the given time and only moves on advance', () => {
  const clock = new ManualClock(1_000);
  assert.equal(clock.now(), 1_000);
  clock.advance(250);
  assert.equal(clock.now(), 1_250);
});

test('ManualClock fires timers in due order', () => {
  const clock = new ManualClock();
  const fired: string[] = [];
  clock.setTimeout(() => fired.push('b'), 20);
  clock.setTimeout(() => fired.push('a'), 10);
  clock.setTimeout(() => fired.push('c'), 30);
  clock.advance(25);
  assert.deepEqual(fired, ['a', 'b']);
  clock.advance(5);
  assert.deepEqual(fired, ['a', 'b', 'c']);
});

test('ManualClock breaks ties by schedule order', () => {
  const clock = new ManualClock();
  const fired: number[] = [];
  for (let i = 0; i < 5; i++) clock.setTimeout(() => fired.push(i), 10);
  clock.advance(10);
  assert.deepEqual(fired, [0, 1, 2, 3, 4]);
});

test('ManualClock reports the time of the timer while it fires', () => {
  const clock = new ManualClock();
  const seen: number[] = [];
  clock.setTimeout(() => seen.push(clock.now()), 7);
  clock.setTimeout(() => seen.push(clock.now()), 19);
  clock.advance(100);
  assert.deepEqual(seen, [7, 19]);
  assert.equal(clock.now(), 100);
});

test('ManualClock fires timers scheduled by other timers within the same advance', () => {
  const clock = new ManualClock();
  const seen: number[] = [];
  clock.setTimeout(() => {
    seen.push(clock.now());
    clock.setTimeout(() => seen.push(clock.now()), 5);
  }, 10);
  clock.advance(20);
  assert.deepEqual(seen, [10, 15]);
});

test('ManualClock.clearTimeout prevents a timer from firing', () => {
  const clock = new ManualClock();
  let fired = false;
  const handle = clock.setTimeout(() => {
    fired = true;
  }, 10);
  clock.clearTimeout(handle);
  clock.advance(50);
  assert.equal(fired, false);
  assert.equal(clock.pendingTimers(), 0);
});

test('ManualClock rejects negative and backwards moves', () => {
  const clock = new ManualClock(10);
  assert.throws(() => clock.advance(-1), RangeError);
  assert.throws(() => clock.advanceTo(5), RangeError);
});

test('ManualClock.runAll drains timers and stops runaway loops', () => {
  const clock = new ManualClock();
  let count = 0;
  clock.setTimeout(() => count++, 10);
  clock.setTimeout(() => count++, 500);
  clock.runAll();
  assert.equal(count, 2);
  assert.equal(clock.now(), 500);

  const loop = (): void => {
    clock.setTimeout(loop, 1);
  };
  loop();
  assert.throws(() => clock.runAll(100), /still busy/);
});

test('SystemClock runs and cancels real timers', async () => {
  const clock = new SystemClock();
  let cancelled = false;
  const handle = clock.setTimeout(() => {
    cancelled = true;
  }, 1);
  clock.clearTimeout(handle);
  await new Promise<void>((resolve) => clock.setTimeout(resolve, 5));
  assert.equal(cancelled, false);
});
