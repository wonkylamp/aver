import assert from "node:assert/strict";
import { test } from "node:test";
import { routeInbound, stripQuoted } from "../src/mail/inbound.ts";
import { agentReplyNotice, openedNotice, replyAddress } from "../src/mail/outbound.ts";
import { TicketStore } from "../src/tickets/store.ts";
import { sequentialTokens } from "../src/tickets/tokens.ts";

function setup() {
  const tickets = new TicketStore({ tokens: sequentialTokens() });
  const t = tickets.open({ requester: "ana@example.com", subject: "Invoice wrong", text: "The total is off" });
  return { tickets, t };
}

test("notifications carry the ticket's reply address", () => {
  const { t } = setup();
  const n = openedNotice(t);
  assert.equal(n.to, "ana@example.com");
  assert.equal(n.replyTo, "reply+t0001@support.relaydesk.test");
  assert.equal(n.subject, "[#1001] Invoice wrong");
  assert.equal(replyAddress(t), n.replyTo);
  const r = agentReplyNotice(t, "Sam", "Fixed it");
  assert.equal(r.subject, "Re: [#1001] Invoice wrong");
  assert.match(r.from, /^"Sam \(Relay Desk\)" <help@support\.relaydesk\.test>$/);
});

test("a reply to the reply address lands on its ticket as the customer's", () => {
  const { tickets, t } = setup();
  const r = routeInbound({ from: "Ana <ana@example.com>", to: [replyAddress(t)], subject: "Re: [#1001] Invoice wrong", text: "Thanks!\n\nOn Mon, Sam wrote:\n> Fixed it" }, tickets);
  assert.equal(r.action, "replied");
  assert.deepEqual(tickets.get(t.id)?.messages.at(-1), { from: "Ana <ana@example.com>", text: "Thanks!", kind: "customer", at: tickets.get(t.id)!.messages.at(-1)!.at });
});

test("a colleague on the thread is a third party", () => {
  const { tickets, t } = setup();
  const r = routeInbound({ from: "cfo@example.com", to: ["ana@example.com"], cc: [replyAddress(t)], subject: "Re: Invoice", text: "Adding context" }, tickets);
  assert.equal(r.action, "replied");
  assert.equal(tickets.get(t.id)?.messages.at(-1)?.kind, "third-party");
});

test("the support domain is matched without regard to case", () => {
  const { tickets, t } = setup();
  const r = routeInbound({ from: "ana@example.com", to: [`reply+${t.replyToken}@Support.RelayDesk.test`], subject: "Re", text: "ok" }, tickets);
  assert.equal(r.action, "replied");
});

test("mail to help@ opens a ticket, stripping Re: and an old ticket tag", () => {
  const { tickets } = setup();
  const r = routeInbound({ from: "Ben <ben@example.com>", to: ["help@support.relaydesk.test"], subject: "Re: [#77] Printer", text: "Jammed" }, tickets);
  assert.equal(r.action, "opened");
  if (r.action !== "opened") return;
  assert.equal(r.ticket.subject, "Printer");
  assert.equal(r.ticket.requester, "ben@example.com");
});

test("an unknown reply token opens a new ticket instead of losing the mail", () => {
  const { tickets } = setup();
  const r = routeInbound({ from: "ana@example.com", to: ["reply+zzzz@support.relaydesk.test"], subject: "Re: [#1001] Invoice wrong", text: "Hello?" }, tickets);
  assert.equal(r.action, "opened");
});

test("bounces and mail not for us are dropped", () => {
  const { tickets } = setup();
  assert.deepEqual(routeInbound({ from: "MAILER-DAEMON@mx.example.com", to: ["help@support.relaydesk.test"], subject: "Undeliverable", text: "" }, tickets), { action: "dropped", reason: "bounce" });
  assert.deepEqual(routeInbound({ from: "ana@example.com", to: ["someone@else.example"], subject: "Hi", text: "" }, tickets), { action: "dropped", reason: "not addressed to support" });
});

test("stripQuoted keeps only the new text", () => {
  assert.equal(stripQuoted("New\r\n> old\r\nmore"), "New\nmore");
  assert.equal(stripQuoted("Top\n\n-----Original Message-----\nFrom: x"), "Top");
});
