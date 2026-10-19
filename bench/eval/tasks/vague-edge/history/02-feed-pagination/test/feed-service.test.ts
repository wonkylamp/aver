import { test } from 'node:test';
import assert from 'node:assert/strict';
import { InMemoryActivityRepository } from '../src/feed/repository.ts';
import { FeedService } from '../src/feed/service.ts';
import { InvalidCursorError, InvalidParameterError } from '../src/errors.ts';
import { activityFactory, teamMember } from '../src/testing/fixtures.ts';
import type { Activity, Viewer } from '../src/types.ts';

function service(activities: Activity[]) {
  return new FeedService(new InMemoryActivityRepository(activities));
}

function walk(feed: FeedService, viewer: Viewer, limit: number, order?: 'asc' | 'desc'): string[][] {
  const pages: string[][] = [];
  let cursor: string | undefined;
  do {
    const page = feed.list(viewer, { limit, cursor, order });
    pages.push(page.items.map((a) => a.id));
    cursor = page.nextCursor ?? undefined;
  } while (cursor !== undefined);
  return pages;
}

test('first page is the newest activities', () => {
  const make = activityFactory();
  const activities = Array.from({ length: 30 }, () => make());
  const page = service(activities).list(teamMember);
  assert.equal(page.items.length, 20);
  assert.equal(page.items[0]!.id, activities[29]!.id);
  assert.notEqual(page.nextCursor, null);
});

test('walking the pages visits every activity once, newest first', () => {
  const make = activityFactory();
  const activities = Array.from({ length: 7 }, () => make());
  const pages = walk(service(activities), teamMember, 3);
  assert.deepEqual(pages, [
    [activities[6]!.id, activities[5]!.id, activities[4]!.id],
    [activities[3]!.id, activities[2]!.id, activities[1]!.id],
    [activities[0]!.id],
  ]);
});

test('oldest first when asked', () => {
  const make = activityFactory();
  const activities = Array.from({ length: 4 }, () => make());
  const pages = walk(service(activities), teamMember, 2, 'asc');
  assert.deepEqual(pages.flat(), activities.map((a) => a.id));
});

test('the last full page has no next cursor', () => {
  const make = activityFactory();
  const activities = Array.from({ length: 4 }, () => make());
  const pages = walk(service(activities), teamMember, 2);
  assert.equal(pages.length, 2);
});

test('hides activities the viewer may not see', () => {
  const make = activityFactory();
  const activities = [
    make(),
    make({ teamId: 'team_b' }),
    make({ visibility: 'private', actorId: 'usr_9' }),
    make({ visibility: 'public', teamId: 'team_c' }),
  ];
  const ids = walk(service(activities), teamMember, 1).flat();
  assert.deepEqual(ids, [activities[3]!.id, activities[0]!.id]);
});

test('an empty feed is one empty page', () => {
  assert.deepEqual(service([]).list(teamMember), { items: [], nextCursor: null });
});

test('validates limit and cursor', () => {
  const feed = service([]);
  for (const limit of [0, 101, 2.5]) {
    assert.throws(() => feed.list(teamMember, { limit }), InvalidParameterError);
  }
  assert.throws(() => feed.list(teamMember, { cursor: 'garbage' }), InvalidCursorError);
});
