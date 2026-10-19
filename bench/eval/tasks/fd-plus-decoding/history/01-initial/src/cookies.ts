// The Cookie request header. Public API: see README.
import { decode } from "./encode.ts";

export function parseCookies(header: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const part of header.split(";")) {
    const at = part.indexOf("=");
    if (at < 0) continue;
    const name = part.slice(0, at).trim();
    if (name === "" || name in out) continue;
    let value = part.slice(at + 1).trim();
    if (value.startsWith('"') && value.endsWith('"') && value.length >= 2) value = value.slice(1, -1);
    try {
      out[name] = decode(value);
    } catch {
      out[name] = value;
    }
  }
  return out;
}
