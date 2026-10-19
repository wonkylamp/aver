// The viewer's only door into the signing code. It verifies receipts with the
// same canonical JSON, id and signing-message rules as aver-core, which
// test/verify.test.ts checks against the cross-language vectors in schemas/.
export { parseStrictJson, CanonicalError } from "./core/canonical.ts";
export type { JsonObject, JsonValue } from "./core/canonical.ts";
export { agentIdOf, fromBase64url, objectSigningMessage, signedObjectId } from "./core/signing.ts";
export { verifyAsync } from "@noble/ed25519";
