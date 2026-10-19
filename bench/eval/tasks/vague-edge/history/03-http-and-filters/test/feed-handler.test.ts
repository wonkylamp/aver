import { test } from 'node:test';
import assert from 'node:assert/strict';
import { InMemoryActivityRepository } from '../src/feed/repository.ts';
import { FeedService } from '../src/feed/service.ts';
import { createFeedHandler } from '../src/http/feed-handler.ts';
import { activityFactory, teamMember } from '../src/testing/fixtures.ts';
import { UserDirectory } from '../src/users/directory.ts';

const users = new UserDirectory([
  { id: 'usr_1', name: 'Ana Lima', handle: 'ana' },
  { id: 'usr_2', name: 'Bo Chen', handle: 'bo' },
]);

function setup() {
  const make = activityFactory();
  const activities = [
    make({ verb: 'created', object: { type: 'doc', id: 'doc_1', title: 'Roadmap' } }),
    make({ verb: 'commented', actorId: 'usr_2', object: { type: 'task', id: 'task_1', title: 'Fix login' } }),
    make({ verb: 'shared' }),
  ];
  const handler = createFeedHandler({ feed: new FeedService(new InMemoryActivityRepository(activities)), users });
  return { handler, activities };
}

type Body = { items: { id: string; summary: string; createdAt: string }[]; nextCursor: string | null };

test('returns summaries and ISO timestamps, newest first', () => {
  const { handler, activities } = setup();
  const response = handler({ viewer: teamMember, query: new URLSearchParams() });
  assert.equal(response.status, 200);
  const body = response.body as Body;
  assert.deepEqual(
    body.items.map((i) => i.id),
    [activities[2]!.id, activities[1]!.id, activities[0]!.id],
  );
  assert.equal(body.items[1]!.summary, 'Bo Chen commented on task "Fix login"');
  assert.equal(body.items[2]!.createdAt, '2024-01-15T09:00:00.000Z');
  assert.equal(body.nextCursor, null);
});

test('follows nextCursor', () => {
  const { handler, activities } = setup();
  const first = handler({ viewer: teamMember, query: new URLSearchParams({ limit: '2' }) }).body as Body;
  assert.equal(first.items.length, 2);
  const second = handler({
    viewer: teamMember,
    query: new URLSearchParams({ limit: '2', cursor: first.nextCursor ?? '' }),
  }).body as Body;
  assert.deepEqual(
    second.items.map((i) => i.id),
    [activities[0]!.id],
  );
});

test('filters by verb and actor', () => {
  const { handler, activities } = setup();
  const byVerb = handler({ viewer: teamMember, query: new URLSearchParams({ verb: 'commented,shared' }) }).body as Body;
  assert.deepEqual(
    byVerb.items.map((i) => i.id),
    [activities[2]!.id, activities[1]!.id],
  );
  const byActor = handler({ viewer: teamMember, query: new URLSearchParams({ actor: 'usr_2' }) }).body as Body;
  assert.deepEqual(
    byActor.items.map((i) => i.id),
    [activities[1]!.id],
  );
});

test('rejects unauthenticated requests', () => {
  const { handler } = setup();
  assert.equal(handler({ viewer: null, query: new URLSearchParams() }).status, 401);
});

test('maps bad input to 400s', () => {
  const { handler } = setup();
  const cursor = handler({ viewer: teamMember, query: new URLSearchParams({ cursor: 'nope' }) });
  assert.deepEqual([cursor.status, (cursor.body as { error: string }).error], [400, 'invalid_cursor']);
  const limit = handler({ viewer: teamMember, query: new URLSearchParams({ limit: '500' }) });
  assert.deepEqual(limit.body, {
    error: 'invalid_parameter',
    parameter: 'limit',
    message: 'limit must be an integer from 1 to 100',
  });
  const order = handler({ viewer: teamMember, query: new URLSearchParams({ order: 'sideways' }) });
  assert.equal((order.body as { parameter: string }).parameter, 'order');
});
