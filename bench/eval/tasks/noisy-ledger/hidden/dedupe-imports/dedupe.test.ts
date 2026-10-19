// docs/importing.md: overlapping statements never duplicate, identical
// transactions are counted, matching uses the bank's payee and is per account.
import assert from "node:assert/strict";
import { test } from "node:test";
import { importStatement } from "../../src/import/importer.ts";
import { Ledger } from "../../src/ledger.ts";

function ledger(): Ledger {
  const l = new Ledger();
  l.addAccount({ id: "cur", name: "Current", kind: "current", currency: "GBP", openingBalance: 0 });
  l.addAccount({ id: "joint", name: "Joint", kind: "current", currency: "GBP", openingBalance: 0 });
  return l;
}

/** A Barclays statement: [DD/MM/YYYY, amount, payee] rows. */
function barclays(rows: Array<[string, string, string]>): string {
  return ["Number,Date,Account,Amount,Subcategory,Memo", ...rows.map(([d, a, p]) => `,${d},20-00-00 1234,${a},Debit,${p}\tON ${d.slice(0, 2)} MAR CLP`)].join("\n") + "\n";
}

const MARCH = barclays([
  ["03/03/2026", "-56.10", "TESCO STORES 2041"],
  ["04/03/2026", "-1200.00", "RENT FLAT 2"],
  ["10/03/2026", "-3.10", "COSTA COFFEE 44120"],
]);

test("importing the same statement twice adds nothing the second time", () => {
  const l = ledger();
  importStatement(l, "cur", MARCH);
  const again = importStatement(l, "cur", MARCH);
  assert.equal(again.added.length, 0);
  assert.deepEqual(again.skipped.map((s) => s.reason), ["duplicate", "duplicate", "duplicate"]);
  assert.equal(l.transactions().length, 3);
});

test("an overlapping statement adds only the new days", () => {
  const l = ledger();
  importStatement(l, "cur", MARCH);
  const r = importStatement(l, "cur", barclays([["27/02/2026", "-20.00", "TFL TRAVEL CH"], ["03/03/2026", "-56.10", "TESCO STORES 2041"], ["04/03/2026", "-1200.00", "RENT FLAT 2"]]));
  assert.deepEqual(r.added.map((t) => t.date), ["2026-02-27"]);
  assert.equal(l.balance("cur"), -(2000 + 5610 + 120000 + 310));
});

test("payees that differ only in case and spacing between downloads still match", () => {
  const l = ledger();
  importStatement(l, "cur", MARCH);
  const r = importStatement(l, "cur", barclays([["03/03/2026", "-56.10", "Tesco Stores  2041"]]));
  assert.equal(r.added.length, 0);
});

test("a renamed transaction is still recognised by the bank's payee", () => {
  const l = ledger();
  importStatement(l, "cur", MARCH);
  const tesco = l.transactions().find((t) => t.amount === -5610)!;
  l.update(tesco.id, { payee: "Weekly shop" });
  const r = importStatement(l, "cur", MARCH);
  assert.equal(r.added.length, 0);
});

test("identical transactions are counted: two already in, three in the statement adds one", () => {
  const l = ledger();
  const coffee: [string, string, string] = ["10/03/2026", "-3.10", "COSTA COFFEE 44120"];
  assert.equal(importStatement(l, "cur", barclays([coffee, coffee])).added.length, 2);
  assert.equal(importStatement(l, "cur", barclays([coffee, coffee])).added.length, 0);
  const third = importStatement(l, "cur", barclays([coffee, coffee, coffee]));
  assert.equal(third.added.length, 1);
  assert.deepEqual(third.skipped.map((s) => s.reason), ["duplicate", "duplicate"]);
  assert.equal(l.balance("cur"), -930);
});

test("two identical payments in one new statement are both imported", () => {
  const l = ledger();
  importStatement(l, "cur", MARCH);
  const coffee: [string, string, string] = ["11/03/2026", "-3.10", "COSTA COFFEE 44120"];
  assert.equal(importStatement(l, "cur", barclays([coffee, coffee])).added.length, 2);
});

test("matching is per account", () => {
  const l = ledger();
  importStatement(l, "cur", MARCH);
  assert.equal(importStatement(l, "joint", MARCH).added.length, 3);
  assert.equal(importStatement(l, "joint", MARCH).added.length, 0);
});

test("a different amount or day is a different transaction", () => {
  const l = ledger();
  importStatement(l, "cur", MARCH);
  const r = importStatement(l, "cur", barclays([["03/03/2026", "-56.11", "TESCO STORES 2041"], ["05/03/2026", "-1200.00", "RENT FLAT 2"]]));
  assert.equal(r.added.length, 2);
});

test("Monzo statements are still matched by the bank's id", () => {
  const l = ledger();
  const monzo = "Transaction ID,Date,Name,Amount,Currency,Notes\ntx_1,01/03/2026,Pret A Manger,-4.20,GBP,\ntx_2,01/03/2026,Pret A Manger,-4.20,GBP,\n";
  assert.equal(importStatement(l, "cur", monzo).added.length, 2);
  assert.equal(importStatement(l, "cur", monzo).added.length, 0);
});
