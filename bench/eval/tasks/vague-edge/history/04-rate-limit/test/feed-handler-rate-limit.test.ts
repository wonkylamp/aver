import { test } from 'node:test';
import assert from 'node:assert/strict';
import { InMemoryActivityRepository } from '../src/feed/repository.ts';
import { FeedService } from '../src/feed/service.ts';
import { createFeedHandler } from '../src/http/feed-handler.ts';
import { RateLimiter } from '../src/rate-limit.ts';
import { teamMember } from '../src/testing/fixtures.ts';
import { UserDirectory } from '../src/users/directory.ts';

test('answers 429 with retry-after once a user runs out of requests', () => {
  let now = 0;
  const handler = createFeedHandler({
    feed: new FeedService(new InMemoryActivityRepository()),
    users: new UserDirectory(),
    limiter: new RateLimiter({ capacity: 2, refillPerSecond: 0.5, now: () => now }),
  });
  const request = { viewer: teamMember, query: new URLSearchParams() };
  assert.equal(handler(request).status, 200);
  assert.equal(handler(request).status, 200);
  const limited = handler(request);
  assert.equal(limited.status, 429);
  assert.equal(limited.headers['retry-after'], '2');
  now += 2_000;
  assert.equal(handler(request).status, 200);
});

test('other users are not affected', () => {
  const handler = createFeedHandler({
    feed: new FeedService(new InMemoryActivityRepository()),
    users: new UserDirectory(),
    limiter: new RateLimiter({ capacity: 1, refillPerSecond: 1, now: () => 0 }),
  });
  assert.equal(handler({ viewer: teamMember, query: new URLSearchParams() }).status, 200);
  assert.equal(handler({ viewer: { userId: 'usr_5', teamIds: [] }, query: new URLSearchParams() }).status, 200);
});
