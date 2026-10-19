import assert from "node:assert/strict";
import { test } from "node:test";
import { TicketError, TicketStore } from "../src/tickets/store.ts";
import { randomTokens, sequentialTokens } from "../src/tickets/tokens.ts";

function store(): TicketStore {
  return new TicketStore({ tokens: sequentialTokens(), clock: () => new Date("2026-02-01T09:00:00Z") });
}

test("open numbers tickets and gives each a reply token", () => {
  const s = store();
  const a = s.open({ requester: "ana@example.com", subject: "Login broken", text: "Help" });
  const b = s.open({ requester: "ben@example.com", subject: "  ", text: "Hi" });
  assert.equal(a.id, 1001);
  assert.equal(b.id, 1002);
  assert.equal(a.replyToken, "t0001");
  assert.equal(b.subject, "(no subject)");
  assert.equal(s.byToken("t0002")?.id, 1002);
  assert.equal(s.byToken("t9999"), undefined);
});

test("the first message is the requester's", () => {
  const t = store().open({ requester: "ana@example.com", subject: "Invoice", text: "Wrong total" });
  assert.deepEqual(t.messages.map((m) => [m.from, m.kind, m.text]), [["ana@example.com", "customer", "Wrong total"]]);
  assert.equal(t.status, "open");
});

test("an agent reply makes the ticket pending, a customer reply reopens it", () => {
  const s = store();
  const t = s.open({ requester: "ana@example.com", subject: "Invoice", text: "Wrong total" });
  s.addMessage(t.id, { from: "Sam", text: "Looking", kind: "agent" });
  assert.equal(s.get(t.id)?.status, "pending");
  s.close(t.id);
  s.addMessage(t.id, { from: "ana@example.com", text: "Still wrong", kind: "customer" });
  assert.equal(s.get(t.id)?.status, "open");
});

test("a third party's message does not reopen a closed ticket", () => {
  const s = store();
  const t = s.open({ requester: "ana@example.com", subject: "Invoice", text: "Wrong total" });
  s.close(t.id);
  s.addMessage(t.id, { from: "cfo@example.com", text: "FYI", kind: "third-party" });
  assert.equal(s.get(t.id)?.status, "closed");
});

test("unknown tickets are errors", () => {
  assert.throws(() => store().addMessage(5, { from: "x", text: "y", kind: "agent" }), TicketError);
  assert.throws(() => store().close(5), TicketError);
});

test("list filters by status and requester", () => {
  const s = store();
  s.open({ requester: "ana@example.com", subject: "A", text: "a" });
  const b = s.open({ requester: "ben@example.com", subject: "B", text: "b" });
  s.close(b.id);
  assert.deepEqual(s.list({ status: "open" }).map((t) => t.subject), ["A"]);
  assert.deepEqual(s.list({ requester: "ben@example.com" }).map((t) => t.subject), ["B"]);
});

test("random tokens are 12 base62 characters and do not repeat", () => {
  const seen = new Set<string>();
  for (let i = 0; i < 500; i++) {
    const t = randomTokens();
    assert.match(t, /^[0-9A-Za-z]{12}$/);
    seen.add(t);
  }
  assert.equal(seen.size, 500);
});
