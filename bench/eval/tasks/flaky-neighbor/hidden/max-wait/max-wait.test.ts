import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ManualClock } from '../../src/clock.ts';
import { debounce, type DebounceOptions } from '../../src/debounce.ts';

type Invocation = [at: number, arg: number];

function setup(wait: number, options: DebounceOptions) {
  const clock = new ManualClock();
  const invocations: Invocation[] = [];
  const debounced = debounce((arg: number) => invocations.push([clock.now(), arg]), wait, { ...options, clock });
  /** Calls the debounced function at each time, passing the time as the argument. */
  const callAt = (times: number[]): void => {
    for (const t of times) {
      clock.advanceTo(t);
      debounced(t);
    }
  };
  return { clock, invocations, debounced, callAt };
}

const every = (from: number, to: number, step: number): number[] => {
  const times: number[] = [];
  for (let t = from; t <= to; t += step) times.push(t);
  return times;
};

test('without maxWait a continuous burst runs only once it goes quiet', () => {
  const { clock, invocations, callAt } = setup(50, {});
  callAt(every(0, 300, 20));
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [[350, 300]]);
});

test('a continuous burst runs every maxWait ms with the latest arguments', () => {
  const { clock, invocations, callAt } = setup(50, { maxWait: 100 });
  callAt(every(0, 385, 35));
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [100, 70],
    [200, 175],
    [300, 280],
    [400, 385],
  ]);
});

test('the trailing call runs only for calls made after the last maxWait invocation', () => {
  const { clock, invocations, callAt } = setup(50, { maxWait: 100 });
  callAt(every(0, 315, 35));
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [100, 70],
    [200, 175],
    [300, 280],
    [365, 315],
  ]);
});

test('a short burst behaves as before and leaves no timers behind', () => {
  const { clock, invocations, callAt } = setup(50, { maxWait: 100 });
  callAt([0, 20]);
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [[70, 20]]);
  assert.equal(clock.pendingTimers(), 0);
});

test('a maxWait smaller than wait is raised to wait', () => {
  const { clock, invocations, callAt } = setup(100, { maxWait: 30 });
  callAt([0, 40, 80, 120, 160, 210]);
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [100, 80],
    [200, 160],
    [300, 210],
  ]);
});

test('a maxWait of zero means the burst runs every wait ms', () => {
  const { clock, invocations, callAt } = setup(50, { maxWait: 0 });
  callAt([0, 20, 40, 60, 80, 110, 130]);
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [50, 40],
    [100, 80],
    [150, 130],
  ]);
});

test('with leading, the leading call counts and the window starts at the burst', () => {
  const { clock, invocations, callAt } = setup(50, { maxWait: 100, leading: true });
  callAt(every(0, 175, 35));
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [0, 0],
    [100, 70],
    [200, 175],
  ]);
});

test('with leading, a lone call is not repeated at maxWait', () => {
  const { clock, invocations, callAt } = setup(50, { maxWait: 100, leading: true });
  callAt([0]);
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [[0, 0]]);
  assert.equal(clock.pendingTimers(), 0);
});

test('maxWait invocations happen even with trailing: false', () => {
  const { clock, invocations, callAt } = setup(50, { maxWait: 100, trailing: false });
  callAt(every(0, 315, 35));
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [100, 70],
    [200, 175],
    [300, 280],
  ]);
});

test('cancel ends the burst, so the next call starts a fresh maxWait window', () => {
  const { clock, invocations, debounced, callAt } = setup(50, { maxWait: 100 });
  callAt([0, 35, 70]);
  clock.advanceTo(80);
  debounced.cancel();
  assert.equal(clock.pendingTimers(), 0);
  callAt(every(90, 230, 35));
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [190, 160],
    [280, 230],
  ]);
});

test('flush ends the burst, so the next call starts a fresh maxWait window', () => {
  const { clock, invocations, debounced, callAt } = setup(50, { maxWait: 100 });
  callAt([0, 35, 70]);
  clock.advanceTo(80);
  debounced.flush();
  assert.equal(clock.pendingTimers(), 0);
  callAt(every(90, 230, 35));
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [80, 70],
    [190, 160],
    [280, 230],
  ]);
});

test('each new burst gets its own maxWait window', () => {
  const { clock, invocations, callAt } = setup(50, { maxWait: 100 });
  callAt([0, 35, 70, 105]);
  clock.advanceTo(250);
  callAt(every(300, 440, 35));
  clock.advanceTo(1_000);
  assert.deepEqual(invocations, [
    [100, 70],
    [155, 105],
    [400, 370],
    [490, 440],
  ]);
});

test('a maxWait invocation does not end the burst', () => {
  const { clock, invocations, debounced, callAt } = setup(50, { maxWait: 100 });
  callAt([0, 35, 70, 95]);
  clock.advanceTo(101);
  assert.deepEqual(invocations, [[100, 95]]);
  assert.equal(debounced.pending(), true);
  clock.advanceTo(145);
  assert.equal(debounced.pending(), false);
  assert.deepEqual(invocations, [[100, 95]]);
});

test('maxWait is validated like wait', () => {
  const fn = (): void => {};
  assert.throws(() => debounce(fn, 10, { maxWait: -1 }), RangeError);
  assert.throws(() => debounce(fn, 10, { maxWait: Number.NaN }), RangeError);
  assert.throws(() => debounce(fn, 10, { maxWait: Number.POSITIVE_INFINITY }), RangeError);
  assert.doesNotThrow(() => debounce(fn, 10, { maxWait: 0 }));
  assert.doesNotThrow(() => debounce(fn, 10, { maxWait: undefined }));
});
