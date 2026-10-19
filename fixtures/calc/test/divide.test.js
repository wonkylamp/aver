const test = require('node:test');
const assert = require('node:assert/strict');
const { divide } = require('../src/math');

test('[crit:div-basic] divide returns the quotient', () => {
  assert.equal(divide(6, 3), 2);
  assert.equal(divide(7, 2), 3.5);
});

test('[crit:div-zero] divide throws on division by zero', () => {
  assert.throws(() => divide(1, 0), /division by zero/);
});
