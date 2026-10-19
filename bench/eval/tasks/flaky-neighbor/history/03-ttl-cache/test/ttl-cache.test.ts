import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ManualClock } from '../src/clock.ts';
import { TtlCache } from '../src/ttl-cache.ts';

test('entries live for the ttl and no longer', () => {
  const clock = new ManualClock();
  const cache = new TtlCache<string, number>({ ttl: 100, clock });
  cache.set('a', 1);
  clock.advance(99);
  assert.equal(cache.get('a'), 1);
  clock.advance(1);
  assert.equal(cache.get('a'), undefined);
  assert.equal(cache.size, 0);
});

test('a per-entry ttl overrides the default', () => {
  const clock = new ManualClock();
  const cache = new TtlCache<string, number>({ ttl: 100, clock });
  cache.set('short', 1, 10);
  cache.set('long', 2);
  clock.advance(50);
  assert.equal(cache.has('short'), false);
  assert.equal(cache.has('long'), true);
});

test('setting a key again refreshes its expiry', () => {
  const clock = new ManualClock();
  const cache = new TtlCache<string, number>({ ttl: 100, clock });
  cache.set('a', 1);
  clock.advance(80);
  cache.set('a', 2);
  clock.advance(80);
  assert.equal(cache.get('a'), 2);
});

test('evicts the least recently used entry past maxEntries', () => {
  const clock = new ManualClock();
  const cache = new TtlCache<string, number>({ ttl: 1_000, maxEntries: 2, clock });
  cache.set('a', 1);
  cache.set('b', 2);
  cache.get('a');
  cache.set('c', 3);
  assert.equal(cache.has('a'), true);
  assert.equal(cache.has('b'), false);
  assert.equal(cache.has('c'), true);
});

test('prune removes only expired entries', () => {
  const clock = new ManualClock();
  const cache = new TtlCache<string, number>({ ttl: 100, clock });
  cache.set('a', 1, 10);
  cache.set('b', 2, 20);
  cache.set('c', 3, 200);
  clock.advance(50);
  assert.equal(cache.prune(), 2);
  assert.equal(cache.size, 1);
});

test('getOrCompute computes once while the entry is live', async () => {
  const clock = new ManualClock();
  const cache = new TtlCache<string, number>({ ttl: 100, clock });
  let computed = 0;
  const compute = async (): Promise<number> => ++computed;
  assert.equal(await cache.getOrCompute('k', compute), 1);
  assert.equal(await cache.getOrCompute('k', compute), 1);
  clock.advance(100);
  assert.equal(await cache.getOrCompute('k', compute), 2);
});

test('rejects invalid options', () => {
  assert.throws(() => new TtlCache({ ttl: -1 }), RangeError);
  assert.throws(() => new TtlCache({ ttl: 10, maxEntries: 0 }), RangeError);
});
