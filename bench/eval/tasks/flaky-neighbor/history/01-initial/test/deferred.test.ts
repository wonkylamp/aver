import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createDeferred } from '../src/deferred.ts';

test('resolves with the first value only', async () => {
  const d = createDeferred<number>();
  assert.equal(d.settled, false);
  d.resolve(1);
  d.resolve(2);
  d.reject(new Error('late'));
  assert.equal(d.settled, true);
  assert.equal(await d.promise, 1);
});

test('rejects with the given reason', async () => {
  const d = createDeferred<number>();
  d.reject(new Error('boom'));
  await assert.rejects(d.promise, /boom/);
});
