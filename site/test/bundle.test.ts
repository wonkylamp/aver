import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { test } from "node:test";
import * as esbuild from "esbuild";
import { browserBundle } from "../bundle.ts";
import { Buffer as Shim } from "../src/shims/buffer.ts";
import type * as Verify from "../src/verify.ts";
import { EXAMPLE_KEY, exampleText, vectors } from "./fixtures.ts";

test("the Buffer shim agrees with Node's Buffer on every call signing.ts makes", () => {
  for (let n = 0; n < 70; n++) {
    const bytes = new Uint8Array(randomBytes(n));
    const b64 = Buffer.from(bytes).toString("base64url");
    assert.equal(Shim.from(bytes).toString("base64url"), b64);
    assert.equal(Shim.from(bytes).toString("hex"), Buffer.from(bytes).toString("hex"));
    assert.deepEqual(new Uint8Array(Shim.from(b64, "base64url")), bytes);
    const other = new Uint8Array(randomBytes(n % 5));
    assert.deepEqual(new Uint8Array(Shim.concat([bytes, other])), new Uint8Array(Buffer.concat([bytes, other])));
    assert.equal(Shim.compare(bytes, other), Buffer.compare(bytes, other));
  }
  for (const text of ["", "a", "task", "tasks", "é", "日本", "😀"]) {
    assert.deepEqual(new Uint8Array(Shim.from(text)), new Uint8Array(Buffer.from(text)));
    assert.equal(Shim.compare(Shim.from(text), Shim.from("b")), Buffer.compare(Buffer.from(text), Buffer.from("b")));
  }
  assert.deepEqual(new Uint8Array(Shim.from("AAE", "base64url")), new Uint8Array(Buffer.from("AAE", "base64url")));
});

test("the browser bundle of the verifier, with its shims, verifies and rejects like the source", async () => {
  const result = await esbuild.build({ ...browserBundle("src/verify.ts"), write: false });
  const code = result.outputFiles[0]!.text;
  assert.doesNotMatch(code, /\bfrom\s*"node:/, "the bundle imports nothing from Node");
  const bundled = (await import(`data:text/javascript;base64,${globalThis.Buffer.from(code).toString("base64")}`)) as typeof Verify;

  const genuine = await bundled.verifyReceipt(exampleText, EXAMPLE_KEY);
  assert.ok(genuine.ok);
  assert.equal(genuine.verification.status, "verified");
  assert.equal(genuine.verification.computedId, "rcpt_qc7nstxxceb4ixqglzpsun7dcfpj4hn53xzu3isoxm763wej2q4q");

  const tampered = await bundled.verifyReceipt(exampleText.replace("-1 !== 5", "-1 !== 6"), EXAMPLE_KEY);
  assert.ok(tampered.ok);
  assert.equal(tampered.verification.status, "signature-invalid");
});

test("the bundled decoder refuses every non-canonical base64url vector", async () => {
  const result = await esbuild.build({ ...browserBundle("src/core.ts"), write: false });
  const code = result.outputFiles[0]!.text;
  const bundled = (await import(`data:text/javascript;base64,${globalThis.Buffer.from(code).toString("base64")}`)) as typeof import("../src/core.ts");
  for (const { text, bytes, hex } of vectors.base64url.accepted) {
    const decoded = bundled.fromBase64url(text, bytes);
    assert.ok(decoded, text);
    assert.equal(globalThis.Buffer.from(decoded).toString("hex"), hex);
  }
  for (const { text, bytes, reason } of vectors.base64url.rejected) {
    assert.equal(bundled.fromBase64url(text, bytes), undefined, `${text} (${reason})`);
  }
});
