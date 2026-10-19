import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compareActivities, sortActivities } from '../src/feed/order.ts';

const items = [
  { id: 'act_3', createdAt: 200 },
  { id: 'act_1', createdAt: 100 },
  { id: 'act_2', createdAt: 200 },
];

test('asc orders by createdAt, then id', () => {
  assert.deepEqual(
    sortActivities(items, 'asc').map((a) => a.id),
    ['act_1', 'act_2', 'act_3'],
  );
});

test('desc is the exact reverse of asc', () => {
  assert.deepEqual(
    sortActivities(items, 'desc').map((a) => a.id),
    ['act_3', 'act_2', 'act_1'],
  );
});

test('compare is zero only for the same position', () => {
  assert.equal(compareActivities(items[0]!, items[0]!, 'asc'), 0);
  assert.ok(compareActivities(items[2]!, items[0]!, 'asc') < 0);
  assert.ok(compareActivities(items[2]!, items[0]!, 'desc') > 0);
});

test('sortActivities does not mutate its input', () => {
  const copy = [...items];
  sortActivities(items, 'asc');
  assert.deepEqual(items, copy);
});
