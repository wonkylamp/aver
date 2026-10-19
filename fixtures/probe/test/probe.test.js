// Sandbox properties observed from inside. Each test passes only when the
// sandbox is hardened as ARCHITECTURE.md section 2.4.1 requires.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');

test('runs as uid and gid 65534', () => {
  assert.equal(process.getuid(), 65534);
  assert.equal(process.getgid(), 65534);
});

test('root filesystem is read-only', () => {
  // /var/tmp is world-writable in the image, so only a read-only mount stops this.
  assert.throws(() => fs.writeFileSync('/var/tmp/aver-probe', 'x'), { code: 'EROFS' });
  const root = fs.readFileSync('/proc/self/mounts', 'utf8').split('\n').find((l) => l.split(' ')[1] === '/');
  assert.match(root.split(' ')[3], /^ro(,|$)/, root);
});

test('scratch directories are writable', () => {
  for (const dir of ['/work', '/tmp', '/aver/out']) {
    fs.writeFileSync(`${dir}/probe.txt`, 'x');
  }
});

test('environment is the generated minimal one', () => {
  console.log(`env: ${JSON.stringify(process.env)}`);
  assert.equal(process.env.HOME, '/tmp');
  assert.equal(process.env.CI, '1');
  assert.equal(process.env.NODE_ENV, 'test');
  // No run sees its index (a rerun must not be able to behave differently).
  assert.equal(process.env.AVER_RUN_ATTEMPT, undefined);
  assert.equal(process.env.AVER_TEST_CANARY, undefined);
});

test('no capabilities are held', () => {
  const status = fs.readFileSync('/proc/self/status', 'utf8');
  for (const field of ['CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb']) {
    const line = status.split('\n').find((l) => l.startsWith(field + ':'));
    assert.match(line, /:\s*0+$/, line);
  }
  assert.match(status, /NoNewPrivs:\s*1/);
});

test("aver's result files are out of reach", () => {
  // The wrapper runs as root and writes the log and the exit code; the
  // check cannot replace, remove or write them, add files beside them, or
  // write the wrapper's stdout, where the result goes.
  const denied = { code: /^(EACCES|EPERM)$/ };
  assert.equal(fs.statSync('/aver').uid, 0);
  assert.throws(() => fs.writeFileSync('/aver/log', 'forged'), denied);
  assert.throws(() => fs.rmSync('/aver/log', { force: true }), denied);
  assert.throws(() => fs.writeFileSync('/aver/code', '0'), denied);
  assert.throws(() => fs.mkdirSync('/aver/forged'), denied);
  assert.throws(() => fs.writeFileSync('/proc/1/fd/1', 'forged'), denied);
  assert.throws(() => process.kill(1, 'SIGKILL'), { code: 'EPERM' });
  assert.equal(fs.statSync('/aver/out').uid, 65534);
});

test('no docker socket or host source is mounted', () => {
  assert.equal(fs.existsSync('/var/run/docker.sock'), false);
  assert.equal(fs.existsSync('/run/docker.sock'), false);
  const mounts = fs.readFileSync('/proc/self/mounts', 'utf8');
  console.log(mounts);
  for (const line of mounts.trim().split('\n')) {
    const [, target, type] = line.split(' ');
    if (target.startsWith('/work') || target.startsWith('/aver')) {
      assert.ok(type === 'tmpfs' || target.endsWith('/node_modules'), line);
    }
  }
});
