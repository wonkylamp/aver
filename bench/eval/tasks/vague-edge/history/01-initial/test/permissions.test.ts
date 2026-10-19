import { test } from 'node:test';
import assert from 'node:assert/strict';
import { canView, visibleTo } from '../src/permissions.ts';
import type { Activity, Viewer } from '../src/types.ts';

const base: Activity = {
  id: 'act_00000001',
  teamId: 'team_a',
  actorId: 'usr_1',
  verb: 'created',
  object: { type: 'doc', id: 'doc_1' },
  createdAt: 1_700_000_000_000,
  visibility: 'team',
};

const member: Viewer = { userId: 'usr_2', teamIds: ['team_a'] };
const outsider: Viewer = { userId: 'usr_3', teamIds: ['team_b'] };

test('public activities are visible to everyone', () => {
  const activity = { ...base, visibility: 'public' as const };
  assert.equal(canView(outsider, activity), true);
});

test('team activities are visible to team members only', () => {
  assert.equal(canView(member, base), true);
  assert.equal(canView(outsider, base), false);
});

test('private activities are visible to the actor only', () => {
  const activity = { ...base, visibility: 'private' as const };
  assert.equal(canView(member, activity), false);
  assert.equal(canView({ userId: 'usr_1', teamIds: [] }, activity), true);
});

test('visibleTo keeps order', () => {
  const list = [
    { ...base, id: 'a', visibility: 'public' as const },
    { ...base, id: 'b', teamId: 'team_b' },
    { ...base, id: 'c' },
  ];
  assert.deepEqual(
    visibleTo(member, list).map((a) => a.id),
    ['a', 'c'],
  );
});
