//! Object signatures (ARCHITECTURE.md §1.1.3).
//!
//! A signed object is a [`Signed<B>`]: a body `B` plus a [`Signature`] that
//! serializes as the body's fields with a `"signature"` field added. The
//! signature covers
//! `"aver-sig:v0\0" || kind || "\0" || canonical_json(body)`, where `kind` is
//! the id prefix, so a signature over one object type cannot be replayed as
//! another. The object id hashes the full object including the signature.
//! There is no unsigned `Signed<B>`: the only constructors are
//! [`Signed::sign`] and parsing.

use std::fmt;

use data_encoding::BASE64URL_NOPAD;
use ed25519_dalek::Signer;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub use ed25519_dalek::{SigningKey, VerifyingKey};

use crate::canonical;
use crate::id::{AgentId, ContentKind};
use crate::object::{sealed, Object, Versioned};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    #[error("signature names key {expected} but was checked against {actual}")]
    KeyMismatch { expected: AgentId, actual: AgentId },
    #[error("signature is malformed")]
    Malformed,
    #[error("signature does not verify")]
    Invalid,
}

/// Decodes unpadded base64url, accepting only the canonical encoding of the
/// bytes: no padding, no characters outside the URL-safe alphabet, no length
/// that is 1 modulo 4, and zero unused low bits in the last character.
/// Otherwise several strings would decode to the same signature or key,
/// so the decoded bytes must re-encode to the input.
pub fn decode_base64url(text: &str) -> Option<Vec<u8>> {
    let bytes = BASE64URL_NOPAD.decode(text.as_bytes()).ok()?;
    (BASE64URL_NOPAD.encode(&bytes) == text).then_some(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureAlg {
    #[serde(rename = "ed25519")]
    Ed25519,
}

/// 64 raw Ed25519 signature bytes, serialized as unpadded base64url.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SignatureBytes(pub [u8; 64]);

impl SignatureBytes {
    pub fn to_base64url(&self) -> String {
        BASE64URL_NOPAD.encode(&self.0)
    }

    pub fn from_base64url(s: &str) -> Result<Self, SignatureError> {
        let bytes = decode_base64url(s).ok_or(SignatureError::Malformed)?;
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| SignatureError::Malformed)
    }
}

impl fmt::Debug for SignatureBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base64url())
    }
}

impl Serialize for SignatureBytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_base64url())
    }
}

impl<'de> Deserialize<'de> for SignatureBytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::from_base64url(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signature {
    pub alg: SignatureAlg,
    pub key: AgentId,
    pub sig: SignatureBytes,
}

/// Verifies a raw Ed25519 signature made by `public_key` over `message`, after
/// checking that `key` names that public key.
pub(crate) fn verify_raw(
    key: &AgentId,
    public_key: &VerifyingKey,
    message: &[u8],
    sig: &SignatureBytes,
) -> Result<(), SignatureError> {
    let actual = AgentId::of_public_key(public_key);
    if *key != actual {
        return Err(SignatureError::KeyMismatch {
            expected: *key,
            actual,
        });
    }
    public_key
        .verify_strict(message, &ed25519_dalek::Signature::from_bytes(&sig.0))
        .map_err(|_| SignatureError::Invalid)
}

/// The body of a signed object type: everything except `signature`.
pub trait SignedBody: Versioned + Serialize + DeserializeOwned {
    type Kind: ContentKind;
}

/// A body plus the signature over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed<B> {
    pub body: B,
    pub signature: Signature,
}

impl<B: SignedBody> Signed<B> {
    /// The bytes a signature over `body` covers.
    pub fn signing_message(body: &B) -> Vec<u8> {
        let value = serde_json::to_value(body).expect("object bodies always serialize to JSON");
        let canonical =
            canonical::to_canonical(&value).expect("object bodies are canonical by construction");
        let mut message = Vec::with_capacity(16 + B::Kind::PREFIX.len() + canonical.len());
        message.extend_from_slice(b"aver-sig:v0\0");
        message.extend_from_slice(B::Kind::PREFIX.as_bytes());
        message.push(0);
        message.extend_from_slice(&canonical);
        message
    }

    pub fn sign(body: B, key: &SigningKey) -> Self {
        let sig = key.sign(&Self::signing_message(&body));
        Self {
            body,
            signature: Signature {
                alg: SignatureAlg::Ed25519,
                key: AgentId::of_public_key(&key.verifying_key()),
                sig: SignatureBytes(sig.to_bytes()),
            },
        }
    }

    /// Checks that the embedded signature names `public_key` and verifies
    /// over the body.
    pub fn verify(&self, public_key: &VerifyingKey) -> Result<(), SignatureError> {
        verify_raw(
            &self.signature.key,
            public_key,
            &Self::signing_message(&self.body),
            &self.signature.sig,
        )
    }
}

impl<B: SignedBody> sealed::Sealed for Signed<B> {}

impl<B: SignedBody> Object for Signed<B> {
    type Kind = B::Kind;
}

impl<B: SignedBody> Serialize for Signed<B> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut value = serde_json::to_value(&self.body).map_err(S::Error::custom)?;
        let Value::Object(map) = &mut value else {
            return Err(S::Error::custom("signed object body must be a JSON object"));
        };
        map.insert(
            "signature".to_owned(),
            serde_json::to_value(&self.signature).map_err(S::Error::custom)?,
        );
        value.serialize(s)
    }
}

impl<'de, B: SignedBody> Deserialize<'de> for Signed<B> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let Value::Object(mut map) = Value::deserialize(d)? else {
            return Err(D::Error::custom("expected a JSON object"));
        };
        let signature = map
            .remove("signature")
            .ok_or_else(|| D::Error::missing_field("signature"))?;
        let signature = Signature::deserialize(signature).map_err(D::Error::custom)?;
        let body = B::deserialize(Value::Object(map)).map_err(D::Error::custom)?;
        Ok(Self { body, signature })
    }
}
