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
  throw new Error('not implemented');
}

module.exports = { add, sub, mul, divide };
