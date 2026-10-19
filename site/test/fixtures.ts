import { readFileSync } from "node:fs";
import { agentKeyFromSeed, base64url, signObject, type AgentKey } from "../src/core/signing.ts";

const vectors = JSON.parse(readFileSync(new URL("../../schemas/vectors/v0.json", import.meta.url), "utf8")) as {
  key: { seed_hex: string; public_key_hex: string; agent_id: string };
  object_signature: { prefix: string; id: string; signed_utf8: string };
  signed_objects: { prefix: string; id: string; signed_utf8: string }[];
  base64url: {
    accepted: { text: string; bytes: number; hex: string }[];
    rejected: { text: string; bytes: number; reason: string }[];
  };
};

export { vectors };

/** The real receipt in examples/, as the CLI printed it, and the id the Rust store gave it. */
export const exampleText = readFileSync(new URL("../examples/calc-fail-own.json", import.meta.url), "utf8");
export const example = JSON.parse(exampleText) as { receipt_id: string; receipt: Record<string, unknown> };
export const EXAMPLE_KEY = "6k8CJxp-0fIP_Cl8v64ZNVg3PNsmrFY4YBpLFLWflOo";

export async function vectorKey(): Promise<AgentKey> {
  return agentKeyFromSeed(Buffer.from(vectors.key.seed_hex, "hex"), "vectors");
}

export function publicKeyText(key: AgentKey): string {
  return base64url(key.publicKey);
}

/** The example receipt's body with `changes` applied, signed by `key` as a verifier would. */
export async function signedReceipt(key: AgentKey, changes: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const { signature: _, ...body } = example.receipt;
  return signObject("rcpt", { ...body, ...changes }, key);
}
