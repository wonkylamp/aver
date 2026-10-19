import { InvalidCursorError } from '../errors.ts';

/** Where the previous page ended. */
export interface CursorPosition {
  createdAt: number;
}

const VERSION = 1;

/**
 * Cursors are opaque to clients. Format v1 is base64url of the JSON
 * {"v":1,"t":<createdAt>}; see docs/api.md before changing it.
 */
export function encodeCursor(position: CursorPosition): string {
  return Buffer.from(JSON.stringify({ v: VERSION, t: position.createdAt })).toString('base64url');
}

export function decodeCursor(cursor: string): CursorPosition {
  let parsed: unknown;
  try {
    parsed = JSON.parse(Buffer.from(cursor, 'base64url').toString('utf8'));
  } catch {
    throw new InvalidCursorError();
  }
  if (typeof parsed !== 'object' || parsed === null) throw new InvalidCursorError();
  const { v, t } = parsed as Record<string, unknown>;
  if (v !== VERSION || typeof t !== 'number' || !Number.isSafeInteger(t)) {
    throw new InvalidCursorError();
  }
  return { createdAt: t };
}
