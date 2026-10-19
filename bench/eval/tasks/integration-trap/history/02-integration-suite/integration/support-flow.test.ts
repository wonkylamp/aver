// End to end: the HTTP API, real reply tokens, and email round trips
// through the simulated provider. Slow by design (the provider delivers in
// batches); CI runs it before every merge.
import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import type { Running } from "./harness.ts";
import { call, eventually, start } from "./harness.ts";

let r: Running;
before(async () => {
  r = await start();
});
after(async () => {
  await r.stop();
});

async function openTicket(requester: string, subject: string): Promise<{ id: number; notice: (typeof r.provider.outbox)[number] }> {
  const res = await call(r, "POST", "/tickets", { requester, subject, text: `${subject}, please help` });
  assert.equal(res.status, 201);
  const notice = r.provider.outbox.findLast((m) => m.to === requester);
  assert.ok(notice, "the requester was notified");
  return { id: res.json.id, notice };
}

async function messages(id: number): Promise<Array<{ from: string; kind: string; text: string }>> {
  return (await call(r, "GET", `/tickets/${id}`)).json.messages;
}

describe("accounts", () => {
  test("a customer signs up and logs in", async () => {
    const s = await call(r, "POST", "/signup", { email: "noor@example.com", name: "Noor", password: "pass phrase 77" });
    assert.equal(s.status, 201);
    const l = await call(r, "POST", "/login", { email: "noor@example.com", password: "pass phrase 77" });
    assert.equal(l.status, 200);
    assert.equal(l.json.id, s.json.id);
    assert.equal((await call(r, "POST", "/login", { email: "noor@example.com", password: "nope nope 1" })).status, 401);
  });

  test("signing up twice with one address is a conflict", async () => {
    await call(r, "POST", "/signup", { email: "omar@example.com", name: "Omar", password: "pass phrase 77" });
    const again = await call(r, "POST", "/signup", { email: "omar@example.com", name: "Omar", password: "pass phrase 78" });
    assert.equal(again.status, 409);
  });
});

describe("email round trips", () => {
  test("opening a ticket mails the requester its reply address", async () => {
    const { id, notice } = await openTicket("pia@example.com", "Cannot export");
    assert.match(notice.replyTo, /^reply\+[0-9A-Za-z]{12}@support\.relaydesk\.test$/);
    assert.equal(notice.subject, `[#${id}] Cannot export`);
  });

  test("the customer's emailed reply lands on the ticket", async () => {
    const { id, notice } = await openTicket("quinn@example.com", "Refund");
    r.provider.reply(notice, "Quinn <quinn@example.com>", "Order 5521, thanks");
    const got = await eventually(r, async () => (await messages(id)).find((m) => m.text === "Order 5521, thanks"));
    assert.ok(got, "the reply is on the ticket");
    assert.equal(got.kind, "customer");
  });

  test("a colleague copied on the reply is recorded as a third party", async () => {
    const { id, notice } = await openTicket("rosa@example.com", "Invoice address");
    r.provider.receive({ from: "finance@example.com", to: ["rosa@example.com"], cc: [notice.replyTo], subject: `Re: ${notice.subject}`, text: "Use the Leeds office" });
    const got = await eventually(r, async () => (await messages(id)).find((m) => m.text === "Use the Leeds office"));
    assert.equal(got?.kind, "third-party");
  });

  test("a reply to a closed ticket reopens it", async () => {
    const { id, notice } = await openTicket("sami@example.com", "Password reset");
    await call(r, "POST", `/tickets/${id}/close`);
    r.provider.reply(notice, "sami@example.com", "It happened again");
    const status = await eventually(r, async () => {
      const t = (await call(r, "GET", `/tickets/${id}`)).json;
      return t.status === "open" ? t.status : undefined;
    });
    assert.equal(status, "open");
  });

  test("an agent's answer and the customer's reply make a thread", async () => {
    const { id } = await openTicket("tara@example.com", "Dark mode");
    const answered = await call(r, "POST", `/tickets/${id}/replies`, { agent: "Sam", text: "Settings > Appearance" });
    assert.equal(answered.json.status, "pending");
    const notice = r.provider.outbox.findLast((m) => m.to === "tara@example.com")!;
    assert.equal(notice.subject, `Re: [#${id}] Dark mode`);
    r.provider.reply(notice, "tara@example.com", "Found it");
    const got = await eventually(r, async () => {
      const ms = await messages(id);
      return ms.length === 3 ? ms : undefined;
    });
    assert.deepEqual(got?.map((m) => m.kind), ["customer", "agent", "customer"]);
  });

  test("mail to help@ opens a ticket and acknowledges it", async () => {
    const before = r.app.tickets.list().length;
    r.provider.receive({ from: "Uma <uma@example.com>", to: ["help@support.relaydesk.test"], subject: "Billing question", text: "Is VAT included?" });
    const opened = await eventually(r, async () => {
      const ts = r.app.tickets.list();
      return ts.length > before ? ts.at(-1) : undefined;
    });
    assert.equal(opened?.subject, "Billing question");
    assert.ok(r.provider.outbox.some((m) => m.to === "uma@example.com" && m.subject === `[#${opened!.id}] Billing question`));
  });
});
