import { test } from 'node:test';
import assert from 'node:assert/strict';
import { UserDirectory } from '../src/users/directory.ts';

const users = () =>
  new UserDirectory([
    { id: 'usr_1', name: 'Ana Lima', handle: 'ana' },
    { id: 'usr_2', name: 'Bo Chen', handle: '@BoC' },
    { id: 'usr_3', name: 'Andre Silva', handle: 'andre', deactivated: true },
  ]);

test('looks users up by id and by handle, ignoring case and @', () => {
  const dir = users();
  assert.equal(dir.get('usr_1')?.name, 'Ana Lima');
  assert.equal(dir.byHandle('@boc')?.id, 'usr_2');
  assert.equal(dir.byHandle('BOC')?.id, 'usr_2');
});

test('rejects duplicate ids and handles', () => {
  const dir = users();
  assert.throws(() => dir.add({ id: 'usr_1', name: 'X', handle: 'x' }), /duplicate user id/);
  assert.throws(() => dir.add({ id: 'usr_9', name: 'X', handle: '@ANA' }), /duplicate handle/);
});

test('displayName hides deactivated and unknown users', () => {
  const dir = users();
  assert.equal(dir.displayName('usr_1'), 'Ana Lima');
  assert.equal(dir.displayName('usr_3'), 'Former member');
  assert.equal(dir.displayName('usr_404'), 'Unknown user');
});

test('search matches handle or name prefixes and skips deactivated users', () => {
  const dir = users();
  assert.deepEqual(
    dir.search('an').map((u) => u.id),
    ['usr_1'],
  );
  assert.deepEqual(
    dir.search('@b').map((u) => u.id),
    ['usr_2'],
  );
  assert.deepEqual(dir.search(''), []);
});
