// Each test is a probe that PASSES only if the sandbox has a network path it
// must not have. Under Aver every one of them must fail; run with network
// enabled, they pass, which shows the probes work.
const test = require('node:test');
const assert = require('node:assert/strict');
const dns = require('node:dns').promises;
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');

const TIMEOUT_MS = 3000;

// Resolves to the outcome of a TCP connect. A refused connection still proves
// the destination is reachable.
function reach(host, port) {
  return new Promise((resolve) => {
    const socket = net.connect({ host, port });
    const done = (outcome) => {
      socket.destroy();
      resolve(outcome);
    };
    socket.setTimeout(TIMEOUT_MS, () => done('timeout'));
    socket.once('connect', () => done('connected'));
    socket.once('error', (err) => done(err.code === 'ECONNREFUSED' ? 'refused' : err.code));
  });
}

function assertReachable(outcome) {
  assert.ok(outcome === 'connected' || outcome === 'refused', `unreachable: ${outcome}`);
}

function defaultGateway() {
  const lines = fs.readFileSync('/proc/net/route', 'utf8').trim().split('\n').slice(1);
  for (const line of lines) {
    const [, destination, gateway] = line.trim().split(/\s+/);
    if (destination === '00000000') {
      const bytes = Buffer.from(gateway, 'hex');
      return [bytes[3], bytes[2], bytes[1], bytes[0]].join('.');
    }
  }
  return null;
}

test('dns resolves example.com', async () => {
  const { address } = await dns.lookup('example.com');
  assert.ok(address);
});

test('tcp reaches 1.1.1.1:443', async () => {
  assertReachable(await reach('1.1.1.1', 443));
});

test('tcp reaches host.docker.internal', async () => {
  assertReachable(await reach('host.docker.internal', 80));
});

test('a default gateway exists and is reachable', async () => {
  const gateway = defaultGateway();
  assert.ok(gateway, 'no default route');
  assertReachable(await reach(gateway, 80));
});

test('a non-loopback interface has an address', () => {
  const names = Object.keys(os.networkInterfaces()).filter((name) => name !== 'lo');
  console.log(`interfaces with addresses: ${JSON.stringify(Object.keys(os.networkInterfaces()))}`);
  assert.ok(names.length > 0, 'only loopback');
});
