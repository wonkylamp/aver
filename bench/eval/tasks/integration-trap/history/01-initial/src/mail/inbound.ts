// The inbound mail router: every message the mail provider receives for our
// support domain arrives here, and lands on a ticket.
import { normalizeAddress, sameAddress } from "../email/address.ts";
import type { Ticket, TicketStore } from "../tickets/store.ts";
import { SUPPORT_DOMAIN } from "./outbound.ts";

export interface InboundMail {
  from: string;
  to: string[];
  cc?: string[];
  subject: string;
  text: string;
}

export type Routed =
  | { action: "replied"; ticket: Ticket }
  | { action: "opened"; ticket: Ticket }
  | { action: "dropped"; reason: string };

const REPLY = /^reply\+([0-9A-Za-z]+)$/;

/** Strips quoted history ("On ... wrote:" and "> " lines) from a reply. */
export function stripQuoted(text: string): string {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const out: string[] = [];
  for (const line of lines) {
    if (/^On .+wrote:\s*$/.test(line) || /^-{2,}\s*Original Message\s*-{2,}$/i.test(line)) break;
    if (line.startsWith(">")) continue;
    out.push(line);
  }
  return out.join("\n").trim();
}

/** Our recipients of a message (to and cc), canonical, in header order. */
function ourRecipients(mail: InboundMail): string[] {
  const out: string[] = [];
  for (const raw of [...mail.to, ...(mail.cc ?? [])]) {
    let addr: string;
    try {
      addr = normalizeAddress(raw);
    } catch {
      continue;
    }
    if (addr.endsWith(`@${SUPPORT_DOMAIN}`)) out.push(addr);
  }
  return out;
}

export function routeInbound(mail: InboundMail, tickets: TicketStore): Routed {
  if (/^(mailer-daemon|postmaster)@/i.test(mail.from.replace(/^.*</, ""))) return { action: "dropped", reason: "bounce" };
  const text = stripQuoted(mail.text);
  const recipients = ourRecipients(mail);
  if (recipients.length === 0) return { action: "dropped", reason: "not addressed to support" };
  for (const to of recipients) {
    const local = to.slice(0, to.lastIndexOf("@"));
    const m = REPLY.exec(local);
    if (!m) continue;
    const ticket = tickets.byToken(m[1]!);
    if (!ticket) continue;
    const kind = sameAddress(mail.from, ticket.requester) ? "customer" : "third-party";
    return { action: "replied", ticket: tickets.addMessage(ticket.id, { from: mail.from, text, kind }) };
  }
  // A reply whose token we do not know (a mangled address, a ticket from the
  // old system) still opens a ticket: mail to support is never lost.
  const subject = mail.subject.replace(/^((re|fwd?):\s*)+/i, "").replace(/^\[#\d+\]\s*/, "");
  let requester: string;
  try {
    requester = normalizeAddress(mail.from);
  } catch {
    return { action: "dropped", reason: "unreadable sender" };
  }
  return { action: "opened", ticket: tickets.open({ requester, subject, text }) };
}
