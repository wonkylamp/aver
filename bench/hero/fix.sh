#!/bin/sh
# The real fix: implement divide, with a test that claims criterion div.
set -eu
cat > src/math.js <<'JS'
function add(a, b) {
  return a + b;
}

function sub(a, b) {
  return a - b;
}

function mul(a, b) {
  return a * b;
}

function divide(a, b) {
  if (b === 0) throw new Error('division by zero');
  return a / b;
}

module.exports = { add, sub, mul, divide };
JS
cat > test/divide.test.js <<'JS'
const test = require('node:test');
const assert = require('node:assert/strict');
const { divide } = require('../src/math');

test('[crit:div] divide returns the quotient and throws on zero', () => {
  assert.equal(divide(7, 2), 3.5);
  assert.throws(() => divide(1, 0), /division by zero/);
});
JS
