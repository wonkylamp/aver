import { test } from 'node:test';
import assert from 'node:assert/strict';
import { relativeTime } from '../src/format/relative-time.ts';
import { summarize, truncate } from '../src/format/summary.ts';
import type { Activity } from '../src/types.ts';
import { UserDirectory } from '../src/users/directory.ts';

const users = new UserDirectory([{ id: 'usr_1', name: 'Ana Lima', handle: 'ana' }]);

const activity: Activity = {
  id: 'act_00000001',
  teamId: 'team_a',
  actorId: 'usr_1',
  verb: 'commented',
  object: { type: 'task', id: 'task_1', title: 'Fix login' },
  createdAt: 0,
  visibility: 'team',
};

test('summarize names the actor, the verb and the object', () => {
  assert.equal(summarize(activity, users), 'Ana Lima commented on task "Fix login"');
  assert.equal(
    summarize({ ...activity, verb: 'mentioned', object: { type: 'doc', id: 'd' } }, users),
    'Ana Lima mentioned you in a doc',
  );
});

test('summarize falls back for unknown actors', () => {
  assert.equal(summarize({ ...activity, actorId: 'usr_x' }, users), 'Unknown user commented on task "Fix login"');
});

test('truncate shortens long titles on character boundaries', () => {
  assert.equal(truncate('short'), 'short');
  assert.equal(truncate('abcdefghij', 5), 'abcd…');
  assert.equal(truncate('😀😀😀😀😀😀', 4), '😀😀😀…');
});

test('relativeTime picks the coarsest sensible unit', () => {
  const now = Date.UTC(2024, 4, 20, 12, 0, 0);
  assert.equal(relativeTime(now - 5_000, now), 'just now');
  assert.equal(relativeTime(now - 60_000, now), '1 minute ago');
  assert.equal(relativeTime(now - 3 * 3_600_000, now), '3 hours ago');
  assert.equal(relativeTime(now - 30 * 3_600_000, now), 'yesterday');
  assert.equal(relativeTime(now - 4 * 86_400_000, now), '4 days ago');
  assert.equal(relativeTime(now - 10 * 86_400_000, now), '2024-05-10');
});

test('relativeTime treats clock skew from the future as now', () => {
  assert.equal(relativeTime(2_000, 1_000), 'just now');
});
