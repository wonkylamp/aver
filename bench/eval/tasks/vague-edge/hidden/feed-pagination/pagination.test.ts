import { test } from 'node:test';
import assert from 'node:assert/strict';
import { InMemoryActivityRepository } from '../../src/feed/repository.ts';
import { FeedService, type FeedQuery } from '../../src/feed/service.ts';
import { createFeedHandler } from '../../src/http/feed-handler.ts';
import { activityFactory, T0, teamMember } from '../../src/testing/fixtures.ts';
import type { Activity, Verb } from '../../src/types.ts';
import { UserDirectory } from '../../src/users/directory.ts';

const MINUTE = 60_000;
const VERBS: Verb[] = ['commented', 'updated', 'shared'];

/**
 * 21 activities over 9 timestamps; most timestamps are shared by several
 * activities (bulk imports). Ids are deliberately not in createdAt order.
 * Some activities are hidden from `teamMember` (other team, or private).
 */
function dataset(): { all: Activity[]; visible: Activity[] } {
  const make = activityFactory();
  const sizes = [1, 3, 1, 5, 2, 1, 4, 1, 3];
  const all: Activity[] = [];
  const visible: Activity[] = [];
  let i = 0;
  sizes.forEach((size, minute) => {
    for (let k = 0; k < size; k++, i++) {
      const hiddenTeam = i % 5 === 4;
      const isPrivate = i % 4 === 3;
      const activity = make({
        id: `act_${String((i * 5) % 21).padStart(8, '0')}`,
        createdAt: T0 + minute * MINUTE,
        verb: VERBS[i % 3] as Verb,
        teamId: hiddenTeam ? 'team_b' : 'team_a',
        visibility: isPrivate ? 'private' : 'team',
        actorId: isPrivate ? 'usr_9' : 'usr_1',
      });
      all.push(activity);
      if (!hiddenTeam && !isPrivate) visible.push(activity);
    }
  });
  return { all, visible };
}

/** The documented order: createdAt, then id; desc is the exact reverse. */
function expected(items: Activity[], order: 'asc' | 'desc'): string[] {
  const sorted = [...items].sort((a, b) => a.createdAt - b.createdAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  if (order === 'desc') sorted.reverse();
  return sorted.map((a) => a.id);
}

function walk(feed: FeedService, query: FeedQuery, first?: string): string[][] {
  const pages: string[][] = [];
  let cursor = first;
  for (let guard = 0; guard < 200; guard++) {
    const page = feed.list(teamMember, { ...query, cursor });
    pages.push(page.items.map((a) => a.id));
    if (page.nextCursor === null) return pages;
    cursor = page.nextCursor;
  }
  throw new Error('pagination did not terminate');
}

function v1Cursor(createdAt: number): string {
  return Buffer.from(JSON.stringify({ v: 1, t: createdAt })).toString('base64url');
}

for (const order of ['desc', 'asc'] as const) {
  test(`${order}: every activity appears exactly once across pages, whatever the page size`, () => {
    const { all, visible } = dataset();
    const feed = new FeedService(new InMemoryActivityRepository(all));
    for (const limit of [1, 2, 3, 4, 5, 7, 20, 100]) {
      const ids = walk(feed, { limit, order }).flat();
      assert.deepEqual(ids, expected(visible, order), `limit ${limit}`);
    }
  });

  test(`${order}: a page boundary inside a run of equal timestamps`, () => {
    const make = activityFactory();
    const tied = ['act_00000003', 'act_00000001', 'act_00000005', 'act_00000002', 'act_00000004'].map((id) =>
      make({ id, createdAt: T0 }),
    );
    const feed = new FeedService(new InMemoryActivityRepository(tied));
    const ids = expected(tied, order);
    assert.deepEqual(walk(feed, { limit: 2, order }), [ids.slice(0, 2), ids.slice(2, 4), ids.slice(4)]);
  });

  test(`${order}: ties are handled together with filters`, () => {
    const { all, visible } = dataset();
    const feed = new FeedService(new InMemoryActivityRepository(all));
    for (const limit of [1, 2, 3]) {
      const ids = walk(feed, { limit, order, filter: { verbs: ['commented', 'shared'] } }).flat();
      const want = expected(
        visible.filter((a) => a.verb === 'commented' || a.verb === 'shared'),
        order,
      );
      assert.deepEqual(ids, want, `limit ${limit}`);
    }
  });

  test(`${order}: a v1 cursor issued before this change still continues after its timestamp`, () => {
    const { all, visible } = dataset();
    const feed = new FeedService(new InMemoryActivityRepository(all));
    // Minute 5 holds exactly one activity, the last one such a client saw.
    const t = T0 + 5 * MINUTE;
    const rest = visible.filter((a) => (order === 'desc' ? a.createdAt < t : a.createdAt > t));
    assert.deepEqual(
      feed.list(teamMember, { order, limit: 100, cursor: v1Cursor(t) }).items.map((a) => a.id),
      expected(rest, order),
    );
    // Continuing from it page by page reaches the end without gaps or repeats.
    assert.deepEqual(walk(feed, { order, limit: 2 }, v1Cursor(t)).flat(), expected(rest, order));
  });
}

test('the HTTP handler pages through ties in both orders', () => {
  const { all, visible } = dataset();
  const handler = createFeedHandler({
    feed: new FeedService(new InMemoryActivityRepository(all)),
    users: new UserDirectory([{ id: 'usr_1', name: 'Ana Lima', handle: 'ana' }]),
  });
  for (const order of ['desc', 'asc'] as const) {
    const ids: string[] = [];
    let cursor: string | null = null;
    for (let guard = 0; guard < 100; guard++) {
      const query = new URLSearchParams({ limit: '2', order });
      if (cursor !== null) query.set('cursor', cursor);
      const response = handler({ viewer: teamMember, query });
      assert.equal(response.status, 200);
      const body = response.body as { items: { id: string }[]; nextCursor: string | null };
      ids.push(...body.items.map((item) => item.id));
      cursor = body.nextCursor;
      if (cursor === null) break;
      assert.match(cursor, /^[A-Za-z0-9_-]+$/);
    }
    assert.deepEqual(ids, expected(visible, order), order);
  }
});

test('the HTTP handler accepts a v1 cursor and still rejects malformed ones', () => {
  const { all } = dataset();
  const handler = createFeedHandler({
    feed: new FeedService(new InMemoryActivityRepository(all)),
    users: new UserDirectory(),
  });
  const ok = handler({ viewer: teamMember, query: new URLSearchParams({ cursor: v1Cursor(T0 + 5 * MINUTE) }) });
  assert.equal(ok.status, 200);
  for (const bad of ['nope', Buffer.from('{"v":1,"t":"x"}').toString('base64url'), Buffer.from('[]').toString('base64url')]) {
    const response = handler({ viewer: teamMember, query: new URLSearchParams({ cursor: bad }) });
    assert.deepEqual([response.status, (response.body as { error: string }).error], [400, 'invalid_cursor'], bad);
  }
});

test('the last page has no next cursor, even when it ends inside a tie', () => {
  const make = activityFactory();
  const tied = [1, 2, 3, 4].map((n) => make({ id: `act_0000000${n}`, createdAt: T0 }));
  const feed = new FeedService(new InMemoryActivityRepository(tied));
  const pages = walk(feed, { limit: 2 });
  assert.equal(pages.length, 2);
  assert.deepEqual(pages.flat(), ['act_00000004', 'act_00000003', 'act_00000002', 'act_00000001']);
});

test('feeds without ties page exactly as before', () => {
  const make = activityFactory();
  const activities = Array.from({ length: 45 }, () => make());
  const feed = new FeedService(new InMemoryActivityRepository(activities));
  const first = feed.list(teamMember);
  assert.equal(first.items.length, 20);
  assert.equal(first.items[0]!.id, activities[44]!.id);
  const pages = walk(feed, {});
  assert.deepEqual(
    pages.map((p) => p.length),
    [20, 20, 5],
  );
  assert.deepEqual(pages.flat(), activities.map((a) => a.id).reverse());
});
