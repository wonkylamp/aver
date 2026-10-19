import { randomBytes, scryptSync, timingSafeEqual } from "node:crypto";

// scrypt with a per-password salt. N is low because sign-in runs on small
// instances; the rate limiter in front of /login carries the rest.
const N = 1024;
const KEYLEN = 32;

export function hashPassword(password: string): string {
  const salt = randomBytes(16);
  const key = scryptSync(password, salt, KEYLEN, { N });
  return `scrypt$${N}$${salt.toString("base64url")}$${key.toString("base64url")}`;
}

export function verifyPassword(password: string, stored: string): boolean {
  const [scheme, n, salt, key] = stored.split("$");
  if (scheme !== "scrypt" || !n || !salt || !key) return false;
  const expected = Buffer.from(key, "base64url");
  const actual = scryptSync(password, Buffer.from(salt, "base64url"), expected.length, { N: Number(n) });
  return timingSafeEqual(actual, expected);
}

export function checkStrength(password: string): string | null {
  if (password.length < 10) return "at least 10 characters";
  if (!/[A-Za-z]/.test(password) || !/[0-9]/.test(password)) return "letters and digits";
  return null;
}
