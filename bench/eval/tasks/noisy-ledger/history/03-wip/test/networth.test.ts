import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { Ledger } from "../src/ledger.ts";
import { netWorth } from "../src/networth.ts";

const manual = { kind: "manual" } as const;

function books(withDebt: boolean): Ledger {
  const l = new Ledger();
  l.addAccount({ id: "cur", name: "Current", kind: "current", currency: "GBP", openingBalance: 150000 });
  l.addAccount({ id: "sav", name: "Savings", kind: "savings", currency: "GBP", openingBalance: 500000 });
  if (withDebt) {
    l.addAccount({ id: "cc", name: "Card", kind: "credit-card", currency: "GBP", openingBalance: 0 });
    l.addAccount({ id: "loan", name: "Car loan", kind: "loan", currency: "GBP", openingBalance: -800000 });
    l.add({ account: "cc", date: "2026-03-05", amount: -45000, payee: "Flights", source: manual });
  }
  l.add({ account: "cur", date: "2026-03-01", amount: -20000, payee: "Rent", source: manual });
  return l;
}

describe("assets only", () => {
  test("sums every account", () => assert.deepEqual(netWorth(books(false), "2026-03-31"), { date: "2026-03-31", assets: 630000, liabilities: 0, net: 630000 }));
  test("on an earlier date", () => assert.equal(netWorth(books(false), "2026-02-28").net, 650000));
  test("an empty ledger is worth nothing", () => assert.equal(netWorth(new Ledger(), "2026-03-31").net, 0));
});

describe("with liabilities", () => {
  test("credit card debt is a liability", () => assert.equal(netWorth(books(true), "2026-03-31").liabilities, 845000));
  test("net is assets minus liabilities", () => assert.equal(netWorth(books(true), "2026-03-31").net, 630000 - 845000));
  test("before the card was used", () => assert.equal(netWorth(books(true), "2026-03-01").liabilities, 800000));
});
