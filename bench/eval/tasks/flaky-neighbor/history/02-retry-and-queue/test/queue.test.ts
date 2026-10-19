import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createDeferred, type Deferred } from '../src/deferred.ts';
import { QueueClearedError } from '../src/errors.ts';
import { TaskQueue } from '../src/queue.ts';
import { settle } from '../src/testing/settle.ts';

function gate(): { task: () => Promise<string>; open: Deferred<string> } {
  const open = createDeferred<string>();
  return { task: () => open.promise, open };
}

test('never runs more than `concurrency` tasks at once', async () => {
  const queue = new TaskQueue({ concurrency: 2 });
  const gates = [gate(), gate(), gate(), gate()];
  const results = gates.map((g) => queue.add(g.task));
  await settle();
  assert.equal(queue.running, 2);
  assert.equal(queue.size, 2);

  gates[0]?.open.resolve('a');
  await settle();
  assert.equal(queue.running, 2);
  assert.equal(queue.size, 1);

  for (const g of gates) g.open.resolve('x');
  assert.deepEqual(await Promise.all(results), ['a', 'x', 'x', 'x']);
});

test('starts tasks in the order they were added', async () => {
  const queue = new TaskQueue();
  const order: number[] = [];
  await Promise.all([1, 2, 3].map((n) => queue.add(async () => order.push(n))));
  assert.deepEqual(order, [1, 2, 3]);
});

test('a failing task rejects its own promise and the queue keeps going', async () => {
  const queue = new TaskQueue();
  const failed = queue.add(async () => {
    throw new Error('bad task');
  });
  const ok = queue.add(async () => 'fine');
  await assert.rejects(failed, /bad task/);
  assert.equal(await ok, 'fine');
});

test('pause holds tasks until resume', async () => {
  const queue = new TaskQueue({ paused: true });
  let ran = false;
  const p = queue.add(async () => {
    ran = true;
  });
  await settle();
  assert.equal(ran, false);
  queue.resume();
  await p;
  assert.equal(ran, true);
});

test('clear rejects waiting tasks but lets running ones finish', async () => {
  const queue = new TaskQueue();
  const running = gate();
  const first = queue.add(running.task);
  const second = queue.add(async () => 'never');
  await settle();
  queue.clear();
  await assert.rejects(second, QueueClearedError);
  running.open.resolve('done');
  assert.equal(await first, 'done');
});

test('onIdle resolves once everything has finished', async () => {
  const queue = new TaskQueue({ concurrency: 3 });
  await queue.onIdle();
  const g = gate();
  void queue.add(g.task);
  let idle = false;
  const p = queue.onIdle().then(() => {
    idle = true;
  });
  await settle();
  assert.equal(idle, false);
  g.open.resolve('x');
  await p;
  assert.equal(idle, true);
});

test('rejects a non-positive concurrency', () => {
  assert.throws(() => new TaskQueue({ concurrency: 0 }), RangeError);
  assert.throws(() => new TaskQueue({ concurrency: 1.5 }), RangeError);
});
