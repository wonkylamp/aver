// Email addresses as the helpdesk handles them: parsed from form fields and
// mail headers, and compared in one canonical form.

export interface Address {
  local: string;
  domain: string;
}

export class AddressError extends Error {
  override name = "AddressError";
}

const LOCAL = /^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+$/;
const LABEL = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/;

/**
 * Parses `addr@domain`, or `Display Name <addr@domain>` as mail headers
 * carry it. Quoted local parts and IP-literal domains are not supported:
 * no customer has ever used one.
 */
export function parseAddress(raw: string): Address {
  const text = raw.trim();
  const angle = /<([^<>]*)>\s*$/.exec(text);
  const addr = (angle ? angle[1]! : text).trim();
  const at = addr.lastIndexOf("@");
  if (at <= 0 || at === addr.length - 1) throw new AddressError(`not an email address: ${JSON.stringify(raw)}`);
  const local = addr.slice(0, at);
  const domain = addr.slice(at + 1);
  if (local.length > 64 || !LOCAL.test(local) || local.startsWith(".") || local.endsWith(".") || local.includes("..")) {
    throw new AddressError(`bad local part in ${JSON.stringify(raw)}`);
  }
  const labels = domain.split(".");
  if (domain.length > 253 || labels.length < 2 || !labels.every((l) => LABEL.test(l)) || /^\d+$/.test(labels.at(-1)!)) {
    throw new AddressError(`bad domain in ${JSON.stringify(raw)}`);
  }
  return { local, domain };
}

export function isAddress(raw: string): boolean {
  try {
    parseAddress(raw);
    return true;
  } catch {
    return false;
  }
}

/**
 * The canonical form of an address: display name and surrounding space
 * dropped, lowercased. Everything that compares or looks up addresses
 * goes through it. Providers treat local parts case-insensitively in
 * practice, and customers do not remember how they typed theirs.
 */
export function normalizeAddress(raw: string): string {
  const { local, domain } = parseAddress(raw);
  return `${local}@${domain}`.toLowerCase();
}

export function sameAddress(a: string, b: string): boolean {
  try {
    return normalizeAddress(a) === normalizeAddress(b);
  } catch {
    return false;
  }
}

/** `"Name" <local@domain>`, quoting the name for a mail header. */
export function formatAddress(addr: string, name?: string): string {
  const { local, domain } = parseAddress(addr);
  if (!name) return `${local}@${domain}`;
  return `"${name.replace(/["\\]/g, (c) => `\\${c}`)}" <${local}@${domain}>`;
}
