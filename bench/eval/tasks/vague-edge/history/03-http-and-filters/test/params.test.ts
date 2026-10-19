import { test } from 'node:test';
import assert from 'node:assert/strict';
import { InvalidParameterError } from '../src/errors.ts';
import { optionalString, parseLimit, parseOrder } from '../src/http/params.ts';

test('parseLimit', () => {
  assert.equal(parseLimit(null), undefined);
  assert.equal(parseLimit(''), undefined);
  assert.equal(parseLimit(' 25 '), 25);
  for (const bad of ['-1', '2.5', 'ten', '1e3']) {
    assert.throws(() => parseLimit(bad), InvalidParameterError, bad);
  }
});

test('parseOrder', () => {
  assert.equal(parseOrder(null), undefined);
  assert.equal(parseOrder('ASC'), 'asc');
  assert.equal(parseOrder('desc'), 'desc');
  assert.throws(() => parseOrder('newest'), InvalidParameterError);
});

test('optionalString treats empty as absent', () => {
  assert.equal(optionalString(null), undefined);
  assert.equal(optionalString(''), undefined);
  assert.equal(optionalString('x'), 'x');
});
