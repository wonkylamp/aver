import { test } from 'node:test';
import assert from 'node:assert/strict';
import { InMemoryActivityRepository } from '../src/feed/repository.ts';
import { activityFactory } from '../src/testing/fixtures.ts';

function seeded(count: number) {
  const make = activityFactory();
  const activities = Array.from({ length: count }, () => make());
  return { repo: new InMemoryActivityRepository(activities), activities };
}

test('rejects duplicate ids', () => {
  const { repo, activities } = seeded(1);
  assert.throws(() => repo.add(activities[0]!), /duplicate activity id/);
});

test('get and delete by id', () => {
  const { repo, activities } = seeded(3);
  assert.equal(repo.get(activities[1]!.id), activities[1]);
  assert.equal(repo.delete(activities[1]!.id), true);
  assert.equal(repo.get(activities[1]!.id), undefined);
  assert.equal(repo.size, 2);
});

test('pages newest first and reports whether more remain', () => {
  const { repo, activities } = seeded(5);
  const first = repo.page({ order: 'desc', limit: 2 });
  assert.deepEqual(
    first.items.map((a) => a.id),
    [activities[4]!.id, activities[3]!.id],
  );
  assert.equal(first.hasMore, true);
});

test('continues after a position', () => {
  const { repo, activities } = seeded(5);
  const page = repo.page({ order: 'asc', limit: 10, after: { createdAt: activities[1]!.createdAt } });
  assert.deepEqual(
    page.items.map((a) => a.id),
    activities.slice(2).map((a) => a.id),
  );
  assert.equal(page.hasMore, false);
});

test('applies the where filter before the limit', () => {
  const { repo, activities } = seeded(6);
  const even = new Set(activities.filter((_, i) => i % 2 === 0).map((a) => a.id));
  const page = repo.page({ order: 'asc', limit: 2, where: (a) => even.has(a.id) });
  assert.deepEqual(
    page.items.map((a) => a.id),
    [activities[0]!.id, activities[2]!.id],
  );
  assert.equal(page.hasMore, true);
});
