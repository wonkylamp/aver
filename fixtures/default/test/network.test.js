const test = require('node:test');
const dns = require('node:dns/promises');

// Fails in every sandbox: checks have no network.
test('the registry resolves', async () => {
  await dns.lookup('registry.npmjs.org');
});
