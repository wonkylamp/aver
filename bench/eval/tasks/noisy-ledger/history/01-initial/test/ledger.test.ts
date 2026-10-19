import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { idDate, newId } from "../src/ids.ts";
import { Ledger, LedgerError } from "../src/ledger.ts";

function ledger(): Ledger {
  const l = new Ledger();
  l.addAccount({ id: "cur", name: "Current", kind: "current", currency: "GBP", openingBalance: 100000 });
  l.addAccount({ id: "cc", name: "Card", kind: "credit-card", currency: "GBP", openingBalance: 0 });
  return l;
}

const manual = { kind: "manual" } as const;

describe("ledger", () => {
  test("accounts are unique", () => {
    const l = ledger();
    assert.throws(() => l.addAccount({ id: "cur", name: "x", kind: "cash", currency: "GBP", openingBalance: 0 }), LedgerError);
    assert.throws(() => l.account("nope"), LedgerError);
    assert.deepEqual(l.accounts().map((a) => a.id), ["cur", "cc"]);
  });
  test("transactions need a known account and whole minor units", () => {
    const l = ledger();
    assert.throws(() => l.add({ account: "nope", date: "2026-03-01", amount: 1, payee: "x", source: manual }), LedgerError);
    assert.throws(() => l.add({ account: "cur", date: "2026-03-01", amount: 1.5, payee: "x", source: manual }), LedgerError);
  });
  test("transactions are listed by date, then in the order added", () => {
    const l = ledger();
    l.add({ account: "cur", date: "2026-03-02", amount: -100, payee: "b", source: manual });
    l.add({ account: "cur", date: "2026-03-01", amount: -100, payee: "a", source: manual });
    l.add({ account: "cur", date: "2026-03-02", amount: -100, payee: "c", source: manual });
    assert.deepEqual(l.transactions().map((t) => t.payee), ["a", "b", "c"]);
  });
  test("filters", () => {
    const l = ledger();
    l.add({ account: "cur", date: "2026-03-01", amount: -100, payee: "a", source: manual });
    l.add({ account: "cc", date: "2026-03-05", amount: -100, payee: "b", source: manual });
    l.add({ account: "cur", date: "2026-03-09", amount: -100, payee: "c", source: manual });
    assert.deepEqual(l.transactions({ account: "cur" }).map((t) => t.payee), ["a", "c"]);
    assert.deepEqual(l.transactions({ from: "2026-03-02", to: "2026-03-09" }).map((t) => t.payee), ["b", "c"]);
  });
  test("balances include the opening balance and stop at the date", () => {
    const l = ledger();
    l.add({ account: "cur", date: "2026-03-01", amount: -2500, payee: "a", source: manual });
    l.add({ account: "cur", date: "2026-03-10", amount: 10000, payee: "b", source: manual });
    assert.equal(l.balance("cur", "2026-03-05"), 97500);
    assert.equal(l.balance("cur"), 107500);
    assert.equal(l.balance("cc"), 0);
  });
  test("update and remove", () => {
    const l = ledger();
    const t = l.add({ account: "cur", date: "2026-03-01", amount: -100, payee: "a", source: manual });
    l.update(t.id, { payee: "Renamed", category: "misc", tags: ["x"] });
    assert.deepEqual([l.get(t.id)?.payee, l.get(t.id)?.category, l.get(t.id)?.tags], ["Renamed", "misc", ["x"]]);
    l.remove(t.id);
    assert.equal(l.get(t.id), undefined);
    assert.throws(() => l.remove(t.id), LedgerError);
  });
});

describe("ids", () => {
  test("an id carries its date", () => {
    const id = newId("2026-03-14");
    assert.match(id, /^tx-20260314-[0-9a-z]{3}$/);
    assert.equal(idDate(id), "2026-03-14");
    assert.equal(idDate("nope"), null);
  });
  test("ids are unique across a busy day", () => {
    const ids = new Set<string>();
    for (let i = 0; i < 180; i++) ids.add(newId("2026-03-14"));
    assert.equal(ids.size, 180);
  });
});
