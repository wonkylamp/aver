const test = require('node:test');
const assert = require('node:assert/strict');
const { pad } = require('../src/format');

test('legacy formatter pads numbers', () => {
  assert.equal(pad(7, 3), '007');
});
