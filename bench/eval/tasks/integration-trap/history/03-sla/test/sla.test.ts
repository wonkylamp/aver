import assert from "node:assert/strict";
import { test } from "node:test";
import { businessMs, firstResponseSla } from "../src/tickets/sla.ts";
import { TicketStore } from "../src/tickets/store.ts";
import { sequentialTokens } from "../src/tickets/tokens.ts";

const H = 3_600_000;
const at = (s: string) => new Date(s);

const cases: Array<[string, string, string, number]> = [
  ["same morning", "2026-03-02T09:00:00Z", "2026-03-02T11:30:00Z", 2.5],
  ["before opening counts from 09:00", "2026-03-02T06:00:00Z", "2026-03-02T10:00:00Z", 1],
  ["after closing counts nothing", "2026-03-02T18:00:00Z", "2026-03-02T23:00:00Z", 0],
  ["overnight", "2026-03-02T16:00:00Z", "2026-03-03T10:00:00Z", 2],
  ["over a weekend", "2026-03-06T16:00:00Z", "2026-03-09T10:00:00Z", 2],
  ["a whole week", "2026-03-02T00:00:00Z", "2026-03-09T00:00:00Z", 40],
  ["backwards is zero", "2026-03-03T10:00:00Z", "2026-03-02T10:00:00Z", 0],
];

for (const [name, from, to, hours] of cases) {
  test(`businessMs: ${name}`, () => {
    assert.equal(businessMs(at(from), at(to)) / H, hours);
  });
}

test("first-response SLA stops at the first agent reply", () => {
  let now = at("2026-03-02T09:00:00Z");
  const tickets = new TicketStore({ tokens: sequentialTokens(), clock: () => now });
  const t = tickets.open({ requester: "ana@example.com", subject: "Help", text: "x" });
  now = at("2026-03-02T15:00:00Z");
  tickets.addMessage(t.id, { from: "Sam", text: "On it", kind: "agent" });
  const sla = firstResponseSla(t, "priority", at("2026-03-05T09:00:00Z"));
  assert.deepEqual(sla, { targetHours: 4, elapsedHours: 6, breached: true, firstResponseAt: at("2026-03-02T15:00:00Z") });
  assert.equal(firstResponseSla(t, "standard", at("2026-03-05T09:00:00Z")).breached, false);
});

test("an unanswered ticket keeps counting", () => {
  const tickets = new TicketStore({ tokens: sequentialTokens(), clock: () => at("2026-03-02T09:00:00Z") });
  const t = tickets.open({ requester: "ana@example.com", subject: "Help", text: "x" });
  const sla = firstResponseSla(t, "standard", at("2026-03-04T13:00:00Z"));
  assert.equal(sla.elapsedHours, 20);
  assert.equal(sla.breached, true);
  assert.equal(sla.firstResponseAt, null);
});
