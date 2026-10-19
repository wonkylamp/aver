import assert from "node:assert/strict";
import { setTimeout as sleep } from "node:timers/promises";
import { describe, test } from "node:test";
import { StatementCache } from "../src/import/cache.ts";
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

describe("the statement cache", () => {
  test("the import reuses the preview's parse", () => {
    const cache = new StatementCache();
    const preview = cache.read(BARCLAYS, undefined, "GBP");
    const r = importStatement(ledger(), "cur", BARCLAYS, { cache });
    assert.equal(preview.length, 2);
    assert.equal(r.added.length, 2);
    assert.deepEqual([cache.hits, cache.misses], [1, 1]);
  });

  test("rows handed out are copies", () => {
    const cache = new StatementCache();
    cache.read(MONZO)[0]!.amount = 1;
    assert.equal(cache.read(MONZO)[0]!.amount, -420);
  });

  test("the oldest entry goes when the cache is full", () => {
    const cache = new StatementCache({ max: 2 });
    cache.read(MONZO);
    cache.read(BARCLAYS);
    cache.read(STARLING);
    assert.equal(cache.size, 2);
    assert.equal(cache.has(MONZO), false);
    assert.equal(cache.has(STARLING), true);
  });

  test("format and currency are part of the key", () => {
    const cache = new StatementCache();
    cache.read(MONZO, "monzo", "GBP");
    assert.equal(cache.has(MONZO, "monzo", "GBP"), true);
    assert.equal(cache.has(MONZO, "monzo", "EUR"), false);
  });

  test("a cached statement expires after its ttl", async () => {
    const cache = new StatementCache({ ttlMs: 200 });
    cache.read(MONZO);
    assert.equal(cache.has(MONZO), true);
    // Roughly the ttl, as between a slow preview and the import.
    await sleep(140 + Math.random() * 200);
    assert.equal(cache.has(MONZO), false);
  });
});
