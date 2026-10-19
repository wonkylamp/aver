import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { importStatement } from "../src/import/importer.ts";
import { Ledger } from "../src/ledger.ts";
import { BARCLAYS, MONZO, REVOLUT, STARLING } from "../src/testing/statements.ts";

function ledger(): Ledger {
  const l = new Ledger();
  l.addAccount({ id: "cur", name: "Current", kind: "current", currency: "GBP", openingBalance: 0 });
  l.addAccount({ id: "joint", name: "Joint", kind: "current", currency: "GBP", openingBalance: 0 });
  return l;
}

describe("importing", () => {
  test("every booked row becomes a transaction with a display payee", () => {
    const l = ledger();
    const r = importStatement(l, "cur", BARCLAYS, { statement: "barclays-march.csv" });
    assert.equal(r.added.length, 2);
    assert.deepEqual(l.transactions().map((t) => [t.date, t.payee, t.amount]), [
      ["2026-03-03", "Tesco Stores", -5610],
      ["2026-03-04", "Rent Flat 2", -120000],
    ]);
    const source = l.transactions()[0]!.source;
    assert.deepEqual(source, { kind: "import", bankId: undefined, payee: "TESCO STORES 2041", statement: "barclays-march.csv" });
  });

  test("pending and reverted rows are skipped and reported", () => {
    const l = ledger();
    const r = importStatement(l, "cur", REVOLUT);
    assert.equal(r.added.length, 2);
    assert.deepEqual(r.skipped.map((s) => s.reason), ["pending", "reverted"]);
  });

  test("a Monzo statement imported twice adds nothing the second time", () => {
    const l = ledger();
    importStatement(l, "cur", MONZO);
    const again = importStatement(l, "cur", MONZO);
    assert.equal(again.added.length, 0);
    assert.deepEqual(again.skipped.map((s) => s.reason), ["duplicate", "duplicate"]);
    assert.equal(l.transactions().length, 2);
  });

  test("two identical card payments on one day are both imported", () => {
    const l = ledger();
    const twoCoffees = `Number,Date,Account,Amount,Subcategory,Memo
,10/03/2026,20-00-00 1234,-3.10,Debit,COSTA COFFEE 44120\tON 09 MAR CLP
,10/03/2026,20-00-00 1234,-3.10,Debit,COSTA COFFEE 44120\tON 09 MAR CLP
`;
    const r = importStatement(l, "cur", twoCoffees);
    assert.equal(r.added.length, 2);
    assert.equal(l.balance("cur"), -620);
  });

  test("the same Monzo transaction can be imported into two accounts", () => {
    const l = ledger();
    importStatement(l, "cur", MONZO);
    const r = importStatement(l, "joint", MONZO);
    assert.equal(r.added.length, 2);
  });

  test("Starling and currencies from the account", () => {
    const l = ledger();
    importStatement(l, "cur", STARLING);
    assert.equal(l.balance("cur"), -8800);
  });
});
