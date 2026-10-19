import { test } from 'node:test';
import assert from 'node:assert/strict';
import { constant, exponential, linear, withFullJitter } from '../src/backoff.ts';

test('constant returns the same delay for every retry', () => {
  const b = constant(250);
  assert.deepEqual([1, 2, 3].map(b), [250, 250, 250]);
});

test('linear grows by step and respects the cap', () => {
  const b = linear(100, 50, 220);
  assert.deepEqual([1, 2, 3, 4].map(b), [100, 150, 200, 220]);
});

test('exponential doubles by default and respects the cap', () => {
  const b = exponential({ initial: 100, max: 1_000 });
  assert.deepEqual([1, 2, 3, 4, 5].map(b), [100, 200, 400, 800, 1_000]);
});

test('exponential accepts a custom factor', () => {
  const b = exponential({ initial: 10, factor: 3 });
  assert.deepEqual([1, 2, 3].map(b), [10, 30, 90]);
});

test('exponential rejects a shrinking factor', () => {
  assert.throws(() => exponential({ initial: 10, factor: 0.5 }), RangeError);
});

test('full jitter scales the delay by the random source', () => {
  const values = [0, 0.5, 0.999];
  let i = 0;
  const b = withFullJitter(constant(1_000), () => values[i++] as number);
  assert.deepEqual([1, 1, 1].map(b), [0, 500, 999]);
});
