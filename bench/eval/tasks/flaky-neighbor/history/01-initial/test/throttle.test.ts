import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ManualClock } from '../src/clock.ts';
import { throttle } from '../src/throttle.ts';

test('runs immediately, then at most once per interval', () => {
  const clock = new ManualClock();
  const at: number[] = [];
  const throttled = throttle(() => at.push(clock.now()), 100, { clock });
  for (let t = 0; t <= 250; t += 10) {
    clock.advanceTo(t);
    throttled();
  }
  clock.advance(500);
  assert.deepEqual(at, [0, 100, 200, 300]);
});

test('the trailing call uses the latest arguments', () => {
  const clock = new ManualClock();
  const seen: number[] = [];
  const throttled = throttle((n: number) => seen.push(n), 50, { clock });
  throttled(1);
  throttled(2);
  throttled(3);
  clock.advance(50);
  assert.deepEqual(seen, [1, 3]);
});

test('leading: false waits for the end of the window', () => {
  const clock = new ManualClock();
  const at: number[] = [];
  const throttled = throttle(() => at.push(clock.now()), 40, { clock, leading: false });
  throttled();
  clock.advance(10);
  throttled();
  clock.advance(100);
  assert.deepEqual(at, [40]);
});

test('trailing: false ignores calls made during the window', () => {
  const clock = new ManualClock();
  const at: number[] = [];
  const throttled = throttle(() => at.push(clock.now()), 40, { clock, trailing: false });
  throttled();
  clock.advance(10);
  throttled();
  clock.advance(100);
  throttled();
  assert.deepEqual(at, [0, 110]);
});

test('cancel clears the window and the trailing call', () => {
  const clock = new ManualClock();
  const at: number[] = [];
  const throttled = throttle(() => at.push(clock.now()), 40, { clock });
  throttled();
  throttled();
  throttled.cancel();
  clock.advance(100);
  throttled();
  assert.deepEqual(at, [0, 100]);
});
