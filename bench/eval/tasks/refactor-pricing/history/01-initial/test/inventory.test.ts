import { test } from "node:test";
import assert from "node:assert/strict";
import { Stock } from "../src/inventory/stock.ts";

test("received stock is available", () => {
  const stock = new Stock();
  stock.receive("ROPE-12", 40);
  stock.receive("ROPE-12", 10);
  assert.equal(stock.available("ROPE-12"), 50);
  assert.equal(stock.available("ROPE-16"), 0);
});

test("a reservation is all or nothing", () => {
  const stock = new Stock();
  stock.receive("ROPE-12", 5);
  stock.receive("CLEAT-150", 1);
  assert.equal(stock.reserve([{ sku: "ROPE-12", qty: 3 }, { sku: "CLEAT-150", qty: 2 }]), false);
  assert.equal(stock.available("ROPE-12"), 5);
  assert.equal(stock.reserve([{ sku: "ROPE-12", qty: 3 }, { sku: "ROPE-12", qty: 2 }]), true);
  assert.equal(stock.available("ROPE-12"), 0);
});

test("released stock is available again", () => {
  const stock = new Stock();
  stock.receive("BRUSH-50", 2);
  stock.reserve([{ sku: "BRUSH-50", qty: 2 }]);
  stock.release("BRUSH-50", 1);
  assert.equal(stock.available("BRUSH-50"), 1);
  assert.throws(() => stock.release("BRUSH-50", 5), /only 1 of BRUSH-50 reserved/);
});

test("receiving nothing is an error", () => {
  assert.throws(() => new Stock().receive("ROPE-12", 0), /cannot receive/);
});
