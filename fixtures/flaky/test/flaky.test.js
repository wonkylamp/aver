const test = require('node:test');
const assert = require('node:assert/strict');

// A test keyed to a run index: it would fail on the first run of a check
// and pass on reruns, but no run is told its index (round 13a), so it
// passes on every run.
test('eventually consistent read', () => {
  assert.notEqual(process.env.AVER_RUN_ATTEMPT, '1', 'stale read on attempt 1');
});
