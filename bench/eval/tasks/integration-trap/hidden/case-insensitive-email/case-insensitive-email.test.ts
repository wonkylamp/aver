import assert from "node:assert/strict";
import { test } from "node:test";
import { AccountError, AccountStore } from "../../src/accounts/store.ts";
import { routeInbound } from "../../src/mail/inbound.ts";
import { openedNotice, replyAddress } from "../../src/mail/outbound.ts";
import { TicketStore } from "../../src/tickets/store.ts";

const pw = "correct horse 42";

/** `token` for the first ticket, then sequential ones. */
function firstThen(token: string): () => string {
  let n = 0;
  return () => (n++ === 0 ? token : `x${String(n).padStart(4, "0")}`);
}

function code(fn: () => unknown): string {
  try {
    fn();
  } catch (err) {
    assert.ok(err instanceof AccountError, String(err));
    return err.code;
  }
  assert.fail("expected an AccountError");
}

test("sign-in ignores the letter case of the whole address", () => {
  const store = new AccountStore();
  const a = store.signup({ email: "Alice.Smith@Example.com", name: "Alice", password: pw });
  assert.equal(store.login("alice.smith@example.com", pw).id, a.id);
  assert.equal(store.login("ALICE.SMITH@EXAMPLE.COM", pw).id, a.id);
  assert.equal(store.findByEmail("Alice <alice.SMITH@example.com>")?.id, a.id);
});

test("an address that differs only in case cannot sign up again", () => {
  const store = new AccountStore();
  store.signup({ email: "Alice.Smith@Example.com", name: "Alice", password: pw });
  assert.equal(code(() => store.signup({ email: "alice.smith@example.com", name: "Alice", password: pw })), "duplicate");
  assert.equal(store.size, 1);
});

test("changing the address ignores case, both for the owner and for others", () => {
  const store = new AccountStore();
  const a = store.signup({ email: "lee@example.com", name: "Lee", password: pw });
  store.signup({ email: "Kim@example.com", name: "Kim", password: pw });
  assert.equal(code(() => store.changeEmail(a.id, "KIM@example.com")), "duplicate");
  store.changeEmail(a.id, "Lee@Example.com");
  assert.equal(store.login("lee@example.com", pw).id, a.id);
  store.changeEmail(a.id, "lee.new@example.com");
  assert.equal(store.findByEmail("LEE@example.com"), undefined);
});

test("the account keeps the address as the customer typed it", () => {
  const store = new AccountStore();
  const a = store.signup({ email: "Alice.Smith@Example.com", name: "Alice", password: pw });
  assert.equal(a.email, "Alice.Smith@Example.com");
  assert.equal(store.login("alice.smith@example.com", pw).email, "Alice.Smith@Example.com");
});

test("a reply with a mixed-case token still lands on its ticket", () => {
  const tickets = new TicketStore({ tokens: firstThen("Qm7XkP2aZr9B") });
  const t = tickets.open({ requester: "ana@example.com", subject: "Refund", text: "Order 5521" });
  const r = routeInbound({ from: "ana@example.com", to: [replyAddress(t)], subject: "Re: [#1001] Refund", text: "Any news?" }, tickets);
  assert.equal(r.action, "replied");
  assert.equal(tickets.list().length, 1, "no second ticket was opened");
  assert.equal(tickets.get(t.id)?.messages.at(-1)?.text, "Any news?");
});

test("a reply copied to a mixed-case token is the third party's on that ticket", () => {
  const tickets = new TicketStore({ tokens: firstThen("aB3dE5gH7jK9") });
  const t = tickets.open({ requester: "ana@example.com", subject: "Invoice", text: "x" });
  const r = routeInbound({ from: "cfo@example.com", to: ["ana@example.com"], cc: [replyAddress(t)], subject: "Re: Invoice", text: "Context" }, tickets);
  assert.equal(r.action, "replied");
  assert.equal(tickets.get(t.id)?.messages.at(-1)?.kind, "third-party");
});

test("notices carry the token exactly", () => {
  const tickets = new TicketStore({ tokens: firstThen("Zz09AaYy18Bb") });
  const t = tickets.open({ requester: "Ana@Example.com", subject: "Hi", text: "x" });
  assert.equal(openedNotice(t).replyTo, "reply+Zz09AaYy18Bb@support.relaydesk.test");
});
