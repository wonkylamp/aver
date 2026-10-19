import { InvalidCursorError } from '../errors.ts';

/**
 * Where the previous page ended. Cursors issued since v2 carry the id of the
 * last activity so that activities sharing its createdAt are not skipped; v1
 * cursors only carry the timestamp.
 */
export interface CursorPosition {
  createdAt: number;
  id?: string;
}

/**
 * Cursors are opaque to clients, but clients persist them, so every version
 * ever issued must keep decoding (docs/api.md):
 *   v1: base64url of {"v":1,"t":<createdAt>}
 *   v2: base64url of {"v":2,"t":<createdAt>,"id":<activity id>}
 */
export function encodeCursor(position: CursorPosition): string {
  const payload =
    position.id === undefined ? { v: 1, t: position.createdAt } : { v: 2, t: position.createdAt, id: position.id };
  return Buffer.from(JSON.stringify(payload)).toString('base64url');
}

export function decodeCursor(cursor: string): CursorPosition {
  let parsed: unknown;
  try {
    parsed = JSON.parse(Buffer.from(cursor, 'base64url').toString('utf8'));
  } catch {
    throw new InvalidCursorError();
  }
  if (typeof parsed !== 'object' || parsed === null) throw new InvalidCursorError();
  const { v, t, id } = parsed as Record<string, unknown>;
  if (typeof t !== 'number' || !Number.isSafeInteger(t)) throw new InvalidCursorError();
  if (v === 1) return { createdAt: t };
  if (v === 2 && typeof id === 'string' && id !== '') return { createdAt: t, id };
  throw new InvalidCursorError();
}
