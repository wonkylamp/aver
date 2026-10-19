// src/core/signing.ts uses Node's Buffer for hex, base64url, UTF-8 and byte
// comparison. esbuild injects this object wherever the browser bundle names
// Buffer. It implements exactly those calls; test/bundle.test.ts checks each one
// against Node's Buffer.
const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const VALUES = new Map([...ALPHABET].map((c, i) => [c, i]));

type Encoding = "hex" | "base64url" | "utf8";

class Bytes extends Uint8Array {
  override toString(encoding: Encoding = "utf8"): string {
    if (encoding === "hex") return Array.from(this, (b) => b.toString(16).padStart(2, "0")).join("");
    if (encoding === "base64url") return encodeBase64url(this);
    return new TextDecoder().decode(this);
  }
}

export const Buffer = {
  from(value: string | Uint8Array, encoding: Encoding = "utf8"): Bytes {
    if (typeof value !== "string") return copy(value);
    if (encoding === "base64url") return decodeBase64url(value);
    if (encoding === "utf8") return copy(new TextEncoder().encode(value));
    throw new TypeError(`unsupported encoding ${encoding}`);
  },

  concat(parts: readonly Uint8Array[]): Bytes {
    const out = new Bytes(parts.reduce((n, p) => n + p.length, 0));
    let offset = 0;
    for (const part of parts) {
      out.set(part, offset);
      offset += part.length;
    }
    return out;
  },

  compare(a: Uint8Array, b: Uint8Array): number {
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) {
      if (a[i] !== b[i]) return a[i]! < b[i]! ? -1 : 1;
    }
    return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
  },
};

function copy(bytes: Uint8Array): Bytes {
  const out = new Bytes(bytes.length);
  out.set(bytes);
  return out;
}

function encodeBase64url(bytes: Uint8Array): string {
  let out = "";
  let buffer = 0;
  let bits = 0;
  for (const byte of bytes) {
    buffer = (buffer << 8) | byte;
    bits += 8;
    while (bits >= 6) {
      bits -= 6;
      out += ALPHABET[(buffer >> bits) & 63];
    }
    buffer &= (1 << bits) - 1;
  }
  if (bits > 0) out += ALPHABET[(buffer << (6 - bits)) & 63];
  return out;
}

// Drops leftover bits as Node does. fromBase64url in signing.ts rejects
// characters outside the alphabet and non-canonical text before and after.
function decodeBase64url(text: string): Bytes {
  const bytes: number[] = [];
  let buffer = 0;
  let bits = 0;
  for (const c of text) {
    const value = VALUES.get(c);
    if (value === undefined) break;
    buffer = (buffer << 6) | value;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((buffer >> bits) & 255);
    }
    buffer &= (1 << bits) - 1;
  }
  return copy(Uint8Array.from(bytes));
}
