import {
  CanonicalError,
  agentIdOf,
  fromBase64url,
  objectSigningMessage,
  parseStrictJson,
  signedObjectId,
  verifyAsync,
  type JsonObject,
} from "./core.ts";
import { ReceiptShapeError, readReceipt, type Receipt } from "./receipt.ts";

export type SignatureCheck =
  | { kind: "valid"; signer: string }
  | { kind: "invalid"; signer: string; reason: string }
  | { kind: "no-key"; signer: string }
  | { kind: "bad-key"; signer: string };

export type Status = "verified" | "signature-invalid" | "id-mismatch" | "key-unreadable" | "key-not-provided";

export interface Verification {
  status: Status;
  /** The rcpt_ id of the receipt's canonical JSON, signature included (§1.1). */
  computedId: string;
  /** The id the input claimed, from a `{receipt_id, receipt}` envelope. */
  claimedId: string | null;
  signature: SignatureCheck;
  receipt: Receipt;
}

export type Outcome = { ok: true; verification: Verification } | { ok: false; error: string };

/**
 * Checks a receipt given as JSON text, either bare or in the
 * `{receipt_id, receipt}` envelope `aver verify --json` prints, against an
 * optional verifier public key (unpadded base64url, 32 bytes).
 */
export async function verifyReceipt(text: string, publicKey?: string): Promise<Outcome> {
  let signed: JsonObject;
  let claimedId: string | null;
  let receipt: Receipt;
  let computedId: string;
  try {
    ({ signed, claimedId } = unwrap(parseStrictJson(text)));
    receipt = readReceipt(signed);
    computedId = signedObjectId("rcpt", signed);
  } catch (e) {
    if (e instanceof CanonicalError) return { ok: false, error: `The text is not strict JSON: ${e.message}.` };
    if (e instanceof ReceiptShapeError) return { ok: false, error: `This is not an Aver receipt: ${e.message}.` };
    throw e;
  }
  const signature = await checkSignature(signed, receipt.signature, publicKey?.trim() ?? "");
  const status = statusOf(signature, computedId, claimedId);
  return { ok: true, verification: { status, computedId, claimedId, signature, receipt } };
}

function unwrap(value: ReturnType<typeof parseStrictJson>): { signed: JsonObject; claimedId: string | null } {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new ReceiptShapeError("the input must be a JSON object");
  }
  const inner = value.receipt;
  if (inner === undefined) return { signed: value, claimedId: null };
  if (inner === null || typeof inner !== "object" || Array.isArray(inner)) {
    throw new ReceiptShapeError("receipt must be an object");
  }
  const id = value.receipt_id;
  if (id !== undefined && typeof id !== "string") throw new ReceiptShapeError("receipt_id must be a string");
  return { signed: inner, claimedId: id ?? null };
}

async function checkSignature(signed: JsonObject, sig: Receipt["signature"], publicKey: string): Promise<SignatureCheck> {
  const signer = sig.key;
  if (publicKey === "") return { kind: "no-key", signer };
  const key = fromBase64url(publicKey, 32);
  if (key === undefined) return { kind: "bad-key", signer };
  const keyAgent = agentIdOf(key);
  if (keyAgent !== signer) {
    return { kind: "invalid", signer, reason: `the key belongs to ${keyAgent}, but the receipt names ${signer} as its signer` };
  }
  const bytes = fromBase64url(sig.sig, 64);
  if (bytes === undefined) return { kind: "invalid", signer, reason: "the signature is not 64 bytes of unpadded base64url" };
  const { signature: _, ...body } = signed;
  const ok = await verifyAsync(bytes, objectSigningMessage("rcpt", body), key, { zip215: false });
  return ok ? { kind: "valid", signer } : { kind: "invalid", signer, reason: "the signature does not match the receipt's contents" };
}

function statusOf(signature: SignatureCheck, computedId: string, claimedId: string | null): Status {
  if (signature.kind === "invalid") return "signature-invalid";
  if (claimedId !== null && claimedId !== computedId) return "id-mismatch";
  if (signature.kind === "bad-key") return "key-unreadable";
  if (signature.kind === "no-key") return "key-not-provided";
  return "verified";
}
