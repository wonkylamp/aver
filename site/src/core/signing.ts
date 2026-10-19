import * as ed from "@noble/ed25519";
import { blake3 } from "@noble/hashes/blake3.js";
import { canonicalBytes } from "./canonical.ts";

const BASE32 = "abcdefghijklmnopqrstuvwxyz234567";
const encoder = new TextEncoder();

export function base32Lower(bytes: Uint8Array): string {
  let out = "";
  let buffer = 0;
  let bits = 0;
  for (const byte of bytes) {
    buffer = (buffer << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out += BASE32[(buffer >> bits) & 31];
    }
    buffer &= (1 << bits) - 1;
  }
  if (bits > 0) out += BASE32[(buffer << (5 - bits)) & 31];
  return out;
}

export function base64url(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64url");
}

/** Decodes unpadded base64url, rejecting anything Buffer would silently skip. */
export function fromBase64url(text: string, expectedLength: number): Uint8Array | undefined {
  if (!/^[A-Za-z0-9_-]*$/.test(text)) return undefined;
  const bytes = new Uint8Array(Buffer.from(text, "base64url"));
  if (bytes.length !== expectedLength || base64url(bytes) !== text) return undefined;
  return bytes;
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  return new Uint8Array(Buffer.concat(parts));
}

/** `prefix + "_" + base32(BLAKE3("aver:" || prefix || "\0" || body))` (§1.1). */
export function objectId(prefix: string, body: Uint8Array): string {
  return `${prefix}_${base32Lower(blake3(concat(encoder.encode(`aver:${prefix}\0`), body)))}`;
}

export function agentIdOf(publicKey: Uint8Array): string {
  return objectId("agent", publicKey);
}

export interface AgentKey {
  readonly agentId: string;
  readonly name: string;
  readonly publicKey: Uint8Array;
  readonly seed: Uint8Array;
}

export async function agentKeyFromSeed(seed: Uint8Array, name: string): Promise<AgentKey> {
  const publicKey = await ed.getPublicKeyAsync(seed);
  return { agentId: agentIdOf(publicKey), name, publicKey, seed };
}

export interface Signature {
  alg: "ed25519";
  key: string;
  sig: string;
}

export type Signed<B extends object> = B & { signature: Signature };

/** `"aver-sig:v0\0" || kind || "\0" || canonical_json(body)` (§1.1.3). */
export function objectSigningMessage(kind: string, body: object): Uint8Array {
  if ("signature" in body) throw new Error("the signed body must not contain a signature field");
  return concat(encoder.encode(`aver-sig:v0\0${kind}\0`), canonicalBytes(body));
}

export async function signObject<B extends object>(kind: string, body: B, key: AgentKey): Promise<Signed<B>> {
  const sig = await ed.signAsync(objectSigningMessage(kind, body), key.seed);
  return { ...body, signature: { alg: "ed25519", key: key.agentId, sig: base64url(sig) } };
}

export function signedObjectId(kind: string, signed: object): string {
  return objectId(kind, canonicalBytes(signed));
}
