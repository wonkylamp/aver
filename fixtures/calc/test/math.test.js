const { describe, it } = require('node:test');
const assert = require('node:assert/strict');
const { add, sub, mul } = require('../src/math');

describe('math', () => {
  it('add returns the sum', () => {
    assert.equal(add(2, 3), 5);
  });

  it('sub returns the difference', () => {
    assert.equal(sub(5, 3), 2);
  });

  it('mul returns the product', () => {
    assert.equal(mul(4, 3), 12);
  });
});
