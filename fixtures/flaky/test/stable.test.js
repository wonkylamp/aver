const test = require('node:test');
const assert = require('node:assert/strict');
const { add } = require('../src/math');

test('add is stable', () => {
  assert.equal(add(1, 1), 2);
});

test('no attempt index is provided', () => {
  assert.equal(process.env.AVER_RUN_ATTEMPT, undefined);
});
