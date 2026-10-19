import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ManualClock } from '../src/clock.ts';
import { debounce } from '../src/debounce.ts';

function recorder(clock: ManualClock) {
  const calls: { at: number; args: unknown[] }[] = [];
  const fn = (...args: unknown[]): void => {
    calls.push({ at: clock.now(), args });
  };
  return { calls, fn };
}

test('runs once after the burst goes quiet', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 50, { clock });

  debounced('a');
  clock.advance(20);
  debounced('b');
  clock.advance(20);
  debounced('c');
  clock.advance(49);
  assert.equal(calls.length, 0);
  clock.advance(1);
  assert.deepEqual(calls, [{ at: 90, args: ['c'] }]);
});

test('each call restarts the wait', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 30, { clock });
  for (let i = 0; i < 10; i++) {
    debounced(i);
    clock.advance(29);
  }
  assert.equal(calls.length, 0);
  clock.advance(1);
  assert.deepEqual(calls, [{ at: 291, args: [9] }]);
});

test('separate bursts each produce a call', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 10, { clock });
  debounced(1);
  clock.advance(100);
  debounced(2);
  clock.advance(100);
  assert.deepEqual(
    calls.map((c) => c.args[0]),
    [1, 2],
  );
});

test('leading runs on the first call and trailing only if more calls came', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 50, { clock, leading: true });

  debounced('only');
  clock.advance(100);
  assert.deepEqual(calls, [{ at: 0, args: ['only'] }]);

  debounced('first');
  clock.advance(10);
  debounced('second');
  clock.advance(100);
  assert.deepEqual(calls.slice(1), [
    { at: 100, args: ['first'] },
    { at: 160, args: ['second'] },
  ]);
});

test('trailing: false with leading: true runs only on the leading edge', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 50, { clock, leading: true, trailing: false });
  debounced(1);
  debounced(2);
  clock.advance(10);
  debounced(3);
  clock.advance(200);
  assert.deepEqual(
    calls.map((c) => c.args[0]),
    [1],
  );
});

test('cancel drops the pending call', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 50, { clock });
  debounced('x');
  assert.equal(debounced.pending(), true);
  debounced.cancel();
  assert.equal(debounced.pending(), false);
  clock.advance(500);
  assert.equal(calls.length, 0);
  assert.equal(clock.pendingTimers(), 0);
});

test('flush runs the pending call immediately', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 50, { clock });
  debounced('x');
  clock.advance(10);
  debounced.flush();
  assert.deepEqual(calls, [{ at: 10, args: ['x'] }]);
  clock.advance(500);
  assert.equal(calls.length, 1);
});

test('flush with nothing pending does nothing', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 50, { clock });
  debounced.flush();
  debounced('x');
  clock.advance(60);
  debounced.flush();
  assert.equal(calls.length, 1);
});

test('wait of zero still defers to a timer', () => {
  const clock = new ManualClock();
  const { calls, fn } = recorder(clock);
  const debounced = debounce(fn, 0, { clock });
  debounced('now');
  assert.equal(calls.length, 0);
  clock.advance(0);
  assert.equal(calls.length, 1);
});

test('rejects an invalid wait', () => {
  assert.throws(() => debounce(() => {}, -1), RangeError);
  assert.throws(() => debounce(() => {}, Number.NaN), RangeError);
  assert.throws(() => debounce(() => {}, Number.POSITIVE_INFINITY), RangeError);
});
