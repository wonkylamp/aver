import { test } from 'node:test';
import assert from 'node:assert/strict';
import { decodeCursor, encodeCursor } from '../src/feed/cursor.ts';
import { InvalidCursorError } from '../src/errors.ts';

test('a cursor decodes to the position it was made from', () => {
  const position = { createdAt: 1_705_309_200_000 };
  assert.deepEqual(decodeCursor(encodeCursor(position)), position);
});

test('cursors are URL-safe', () => {
  const cursor = encodeCursor({ createdAt: 8_640_000_000_000 });
  assert.match(cursor, /^[A-Za-z0-9_-]+$/);
});

test('rejects cursors that are not ours', () => {
  for (const bad of ['', 'not-a-cursor', Buffer.from('[1,2]').toString('base64url'), Buffer.from('{"v":1,"t":"x"}').toString('base64url')]) {
    assert.throws(() => decodeCursor(bad), InvalidCursorError, `accepted ${JSON.stringify(bad)}`);
  }
});
