import { formatAddress } from "../email/address.ts";
import type { Ticket } from "../tickets/store.ts";

export const SUPPORT_DOMAIN = "support.relaydesk.test";
export const HELP_ADDRESS = `help@${SUPPORT_DOMAIN}`;

export interface OutboundMail {
  from: string;
  to: string;
  replyTo: string;
  subject: string;
  text: string;
}

export interface Mailer {
  send(mail: OutboundMail): Promise<void>;
}

/** The address a customer replies to; the inbound router maps it back to the ticket. */
export function replyAddress(ticket: Ticket): string {
  return `reply+${ticket.replyToken}@${SUPPORT_DOMAIN}`;
}

export function subjectLine(ticket: Ticket): string {
  return `[#${ticket.id}] ${ticket.subject}`;
}

export function openedNotice(ticket: Ticket): OutboundMail {
  return {
    from: formatAddress(HELP_ADDRESS, "Relay Desk Support"),
    to: ticket.requester,
    replyTo: replyAddress(ticket),
    subject: subjectLine(ticket),
    text: `We received your request and will get back to you soon.\n\nReply to this email to add to ticket #${ticket.id}.`,
  };
}

export function agentReplyNotice(ticket: Ticket, agentName: string, text: string): OutboundMail {
  return {
    from: formatAddress(HELP_ADDRESS, `${agentName} (Relay Desk)`),
    to: ticket.requester,
    replyTo: replyAddress(ticket),
    subject: `Re: ${subjectLine(ticket)}`,
    text: `${text}\n\n-- \nReply to this email to answer ${agentName}.`,
  };
}
