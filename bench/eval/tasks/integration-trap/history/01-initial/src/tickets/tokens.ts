import { randomInt } from "node:crypto";

const ALPHABET = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/** Makes the token in a ticket's reply address. */
export type TokenSource = () => string;

/** 12 random base62 characters (about 71 bits): not guessable, short enough for a mail client. */
export const randomTokens: TokenSource = () => {
  let out = "";
  for (let i = 0; i < 12; i++) out += ALPHABET[randomInt(62)];
  return out;
};

/** Predictable tokens for tests: t0001, t0002, ... */
export function sequentialTokens(prefix = "t"): TokenSource {
  let n = 0;
  return () => `${prefix}${String(++n).padStart(4, "0")}`;
}
