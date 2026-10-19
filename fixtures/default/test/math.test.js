const { describe, it } = require('node:test');
const assert = require('node:assert/strict');
const { add, mul } = require('../dist/index.js');

describe('math', () => {
  it('add returns the sum', () => {
    assert.equal(add(2, 3), 5);
  });

  it('mul returns the product', () => {
    assert.equal(mul(4, 3), 12);
  });
});
