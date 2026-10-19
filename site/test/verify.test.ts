import assert from "node:assert/strict";
import { test } from "node:test";
import { parseStrictJson, signedObjectId } from "../src/core.ts";
import { verifyReceipt } from "../src/verify.ts";
import { EXAMPLE_KEY, example, exampleText, publicKeyText, signedReceipt, vectorKey, vectors } from "./fixtures.ts";

async function verified(text: string, key?: string) {
  const outcome = await verifyReceipt(text, key);
  assert.ok(outcome.ok, outcome.ok ? "" : outcome.error);
  return outcome.verification;
}

test("the viewer's id function reproduces the Rust signed-object vector", () => {
  const { prefix, id, signed_utf8 } = vectors.object_signature;
  assert.equal(signedObjectId(prefix, parseStrictJson(signed_utf8) as object), id);
});

test("the real example receipt verifies and gets the rcpt_ id the Rust store gave it", async () => {
  const v = await verified(exampleText, EXAMPLE_KEY);
  assert.equal(v.status, "verified");
  assert.equal(v.computedId, "rcpt_qc7nstxxceb4ixqglzpsun7dcfpj4hn53xzu3isoxm763wej2q4q");
  assert.equal(v.claimedId, example.receipt_id);
  assert.equal(v.receipt.verdict, "FAIL_OWN");
});

test("a receipt signed with the vectors' seed verifies, bare and wrapped", async () => {
  const key = await vectorKey();
  assert.equal(key.agentId, vectors.key.agent_id);
  const receipt = await signedReceipt(key);
  const id = signedObjectId("rcpt", receipt);

  const bare = await verified(JSON.stringify(receipt), publicKeyText(key));
  assert.equal(bare.status, "verified");
  assert.equal(bare.computedId, id);
  assert.deepEqual(bare.signature, { kind: "valid", signer: vectors.key.agent_id });

  const wrapped = await verified(JSON.stringify({ receipt_id: id, receipt }, null, 2), publicKeyText(key));
  assert.equal(wrapped.status, "verified");
});

test("one changed character makes the signature invalid", async () => {
  const key = await vectorKey();
  const text = JSON.stringify(await signedReceipt(key));
  const tampered = text.replace("-1 !== 5", "-1 !== 6");
  assert.notEqual(tampered, text);
  assert.equal(tampered.length, text.length);

  const v = await verified(tampered, publicKeyText(key));
  assert.equal(v.status, "signature-invalid");
  assert.equal(v.signature.kind, "invalid");
});

test("a tampered receipt inside its envelope reports the bad signature and the id it really has", async () => {
  const key = await vectorKey();
  const receipt = await signedReceipt(key);
  const claimed = signedObjectId("rcpt", receipt);
  const v = await verified(JSON.stringify({ receipt_id: claimed, receipt: { ...receipt, verdict: "PASS" } }), publicKeyText(key));
  assert.equal(v.status, "signature-invalid");
  assert.notEqual(v.computedId, claimed);
});

test("a key other than the signer's is rejected", async () => {
  const key = await vectorKey();
  const v = await verified(JSON.stringify(await signedReceipt(key)), EXAMPLE_KEY);
  assert.equal(v.status, "signature-invalid");
  assert.match(v.signature.kind === "invalid" ? v.signature.reason : "", /the key belongs to agent_soidwj/);
});

test("a valid signature under the wrong claimed id is an id mismatch", async () => {
  const v = await verified(JSON.stringify({ ...example, receipt_id: "rcpt_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }), EXAMPLE_KEY);
  assert.equal(v.status, "id-mismatch");
  assert.equal(v.signature.kind, "valid");
});

test("without a key the id is computed and the signature is left unchecked", async () => {
  const none = await verified(exampleText);
  assert.equal(none.status, "key-not-provided");
  assert.equal(none.computedId, example.receipt_id);
  assert.equal((await verified(exampleText, "  ")).status, "key-not-provided");
  assert.equal((await verified(exampleText, "not-a-key")).status, "key-unreadable");
  assert.equal((await verified(exampleText, `${EXAMPLE_KEY}=`)).status, "key-unreadable");
});

test("input that is not a strict receipt is refused with a reason", async () => {
  const cases: [string, RegExp][] = [
    ["not json", /^The text is not strict JSON: invalid literal/],
    ['{"receipt": {"schema": "aver.receipt/v0", "n": 1.5}}', /floating point/],
    [JSON.stringify({ ...example.receipt, schema: "aver.task/v0" }), /aver\.receipt\/v0/],
    ['{"a": 1, "a": 2}', /duplicate key/],
    [JSON.stringify({ ...example.receipt, "é": 1 }), /is not ASCII/],
    ["[]", /^This is not an Aver receipt: the input must be a JSON object/],
  ];
  for (const [input, reason] of cases) {
    const outcome = await verifyReceipt(input, EXAMPLE_KEY);
    assert.equal(outcome.ok, false, input);
    assert.match(outcome.ok ? "" : outcome.error, reason, input);
  }
});

test("every receipt version the Rust types sign verifies in the viewer, v2 included", async () => {
  const key = Buffer.from(vectors.key.public_key_hex, "hex").toString("base64url");
  const receipts = vectors.signed_objects.filter((o) => o.prefix === "rcpt");
  const schemas = [];
  for (const { id, signed_utf8 } of receipts) {
    const v = await verified(JSON.stringify({ receipt_id: id, receipt: JSON.parse(signed_utf8) }), key);
    assert.equal(v.status, "verified");
    assert.equal(v.computedId, id);
    schemas.push(v.receipt.schema);
  }
  assert.deepEqual(schemas, ["aver.receipt/v0", "aver.receipt/v1", "aver.receipt/v2", "aver.receipt/v3"]);
});
