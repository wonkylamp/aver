#!/bin/sh
# Builds the demo repository at $1: fixtures/calc without divide's tests, four
# npm dependencies from the local npm cache (offline), committed with node_modules
# installed and ignored.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dest=$1
rm -rf "$dest"
mkdir -p "$dest"
cp -R "$here/../../fixtures/calc/src" "$here/../../fixtures/calc/test" "$dest/"
rm "$dest/test/divide.test.js"
cd "$dest"
cat > package.json <<'JSON'
{
  "name": "calc",
  "version": "1.0.0",
  "private": true,
  "scripts": {
    "test": "node --test"
  },
  "dependencies": {
    "lodash": "4.18.1",
    "ms": "2.1.3",
    "picocolors": "1.1.1",
    "semver": "7.8.5"
  }
}
JSON
cat > src/format.js <<'JS'
const ms = require('ms');
const semver = require('semver');
const pc = require('picocolors');
const { sum } = require('lodash');

function duration(n) { return ms(n); }
function newer(a, b) { return semver.gt(a, b); }
function total(xs) { return sum(xs); }
function label(s) { return pc.bold(s); }

module.exports = { duration, newer, total, label };
JS
cat > test/format.test.js <<'JS'
const { it } = require('node:test');
const assert = require('node:assert/strict');
const { duration, newer, total } = require('../src/format');

it('duration formats milliseconds', () => {
  assert.equal(duration(60000), '1m');
});

it('newer compares versions', () => {
  assert.equal(newer('1.2.0', '1.1.9'), true);
});

it('total sums a list', () => {
  assert.equal(total([1, 2, 3]), 6);
});
JS
printf 'node_modules/\n' > .gitignore
npm install --offline --no-audit --no-fund --silent >/dev/null
git init -q -b main
git config user.name demo
git config user.email demo@example.com
git add -A
git commit -qm "calc with four npm dependencies"
aver init --yes >/dev/null 2>&1
git add aver.toml
git commit -qm "aver.toml from aver init"
