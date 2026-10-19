import { test } from 'node:test';
import assert from 'node:assert/strict';
import { matchesFilter, parseVerbList } from '../src/feed/filters.ts';
import { InvalidParameterError } from '../src/errors.ts';
import { activityFactory } from '../src/testing/fixtures.ts';

test('an empty filter matches everything', () => {
  const make = activityFactory();
  assert.equal(matchesFilter(make()), true);
  assert.equal(matchesFilter(make(), { verbs: [] }), true);
});

test('filters by verb and by actor', () => {
  const make = activityFactory();
  const comment = make({ verb: 'commented', actorId: 'usr_7' });
  assert.equal(matchesFilter(comment, { verbs: ['commented', 'shared'] }), true);
  assert.equal(matchesFilter(comment, { verbs: ['shared'] }), false);
  assert.equal(matchesFilter(comment, { actorId: 'usr_7' }), true);
  assert.equal(matchesFilter(comment, { actorId: 'usr_8', verbs: ['commented'] }), false);
});

test('parseVerbList reads a comma-separated list', () => {
  assert.equal(parseVerbList(null), undefined);
  assert.equal(parseVerbList('  '), undefined);
  assert.deepEqual(parseVerbList('commented, shared,commented'), ['commented', 'shared']);
});

test('parseVerbList rejects unknown verbs', () => {
  assert.throws(() => parseVerbList('commented,liked'), InvalidParameterError);
});
