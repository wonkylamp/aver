import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createIdGenerator, hasPrefix } from '../src/ids.ts';

test('generated ids sort as strings in creation order', () => {
  const next = createIdGenerator('act', 30);
  const ids = Array.from({ length: 50 }, () => next());
  assert.deepEqual([...ids].sort(), ids);
  assert.equal(ids[0], 'act_0000000u');
});

test('rejects bad prefixes', () => {
  assert.throws(() => createIdGenerator('Act'));
  assert.throws(() => createIdGenerator(''));
});

test('hasPrefix checks the prefix and separator', () => {
  assert.equal(hasPrefix('act_00000001', 'act'), true);
  assert.equal(hasPrefix('action_1', 'act'), false);
});
