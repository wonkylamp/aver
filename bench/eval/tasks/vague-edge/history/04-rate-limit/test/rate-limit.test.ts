import { test } from 'node:test';
import assert from 'node:assert/strict';
import { RateLimiter } from '../src/rate-limit.ts';

function limiter(capacity = 3, refillPerSecond = 1) {
  let now = 0;
  const rl = new RateLimiter({ capacity, refillPerSecond, now: () => now });
  return { rl, advance: (ms: number) => (now += ms) };
}

test('allows a burst up to capacity, then refuses', () => {
  const { rl } = limiter();
  assert.deepEqual([1, 2, 3, 4].map(() => rl.tryTake('u')), [true, true, true, false]);
});

test('keys have separate buckets', () => {
  const { rl } = limiter(1);
  assert.equal(rl.tryTake('a'), true);
  assert.equal(rl.tryTake('b'), true);
  assert.equal(rl.tryTake('a'), false);
});

test('refills over time and reports when to retry', () => {
  const { rl, advance } = limiter(2, 2);
  rl.tryTake('u');
  rl.tryTake('u');
  assert.equal(rl.retryAfter('u'), 500);
  advance(500);
  assert.equal(rl.retryAfter('u'), 0);
  assert.equal(rl.tryTake('u'), true);
  assert.equal(rl.tryTake('u'), false);
});

test('never refills past capacity', () => {
  const { rl, advance } = limiter(2, 10);
  advance(60_000);
  assert.deepEqual([1, 2, 3].map(() => rl.tryTake('u')), [true, true, false]);
});

test('sweep drops full buckets only', () => {
  const { rl, advance } = limiter(2, 1);
  rl.tryTake('busy');
  rl.tryTake('idle');
  advance(1_000);
  rl.tryTake('busy');
  assert.equal(rl.sweep(), 1);
});

test('rejects nonsense settings', () => {
  assert.throws(() => new RateLimiter({ capacity: 0, refillPerSecond: 1 }), RangeError);
  assert.throws(() => new RateLimiter({ capacity: 1, refillPerSecond: 0 }), RangeError);
});
