//! Typed identifiers (ARCHITECTURE.md §1.1).
//!
//! Content ids are `<prefix>_<52 lowercase base32 chars>`, the BLAKE3 hash of
//! `"aver:" || prefix || "\0" || body`. Each kind is its own type
//! (`TreeId` cannot be passed where a `SnapshotId` is expected), all sharing
//! [`Id`]. Only this crate hashes: blobs via [`BlobId::of_bytes`], agents via
//! [`AgentId::of_public_key`], everything else via [`crate::Object::id`].
//!
//! Live ids are `<prefix>_<ULID>` (canonical uppercase Crockford form) for
//! rows with a lifecycle. Parsing rejects a wrong prefix, wrong length,
//! non-canonical encoding or trailing bits.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::LazyLock;

use data_encoding::{Encoding, Specification};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ulid::Ulid;

use crate::ParseError;

/// A kind of content-addressed value. `PREFIX` is both the textual prefix and
/// the hash domain.
pub trait ContentKind: 'static {
    const PREFIX: &'static str;
}

/// A kind of live (ULID) identifier.
pub trait LiveKind: 'static {
    const PREFIX: &'static str;
}

/// Kind markers. Use the `*Id` aliases instead of naming these directly.
pub mod kind {
    macro_rules! kinds {
        ($trait:ident: $($name:ident = $prefix:literal),* $(,)?) => {$(
            #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
            pub enum $name {}
            impl super::$trait for $name {
                const PREFIX: &'static str = $prefix;
            }
        )*};
    }
    kinds!(ContentKind:
        Blob = "blob",
        Tree = "tree",
        Env = "env",
        Snapshot = "snap",
        Transition = "tran",
        Task = "task",
        Run = "run",
        Receipt = "rcpt",
        AttemptRecord = "atr",
        Ledger = "ledg",
        Agent = "agent",
        Contract = "ctr",
    );
    kinds!(LiveKind:
        Workspace = "ws",
        Attempt = "att",
        Lease = "lease",
    );
}

pub type BlobId = Id<kind::Blob>;
pub type TreeId = Id<kind::Tree>;
pub type EnvId = Id<kind::Env>;
pub type SnapshotId = Id<kind::Snapshot>;
pub type TransitionId = Id<kind::Transition>;
pub type TaskId = Id<kind::Task>;
pub type RunId = Id<kind::Run>;
pub type ReceiptId = Id<kind::Receipt>;
pub type AttemptRecordId = Id<kind::AttemptRecord>;
pub type LedgerId = Id<kind::Ledger>;
pub type AgentId = Id<kind::Agent>;
pub type ContractId = Id<kind::Contract>;

pub type WorkspaceId = LiveId<kind::Workspace>;
pub type AttemptId = LiveId<kind::Attempt>;
pub type LeaseId = LiveId<kind::Lease>;

const HASH_CHARS: usize = 52;

static BASE32_LOWER: LazyLock<Encoding> = LazyLock::new(|| {
    let mut spec = Specification::new();
    spec.symbols.push_str("abcdefghijklmnopqrstuvwxyz234567");
    spec.encoding().expect("valid base32 specification")
});

/// A content id of kind `K`.
pub struct Id<K> {
    bytes: [u8; 32],
    kind: PhantomData<fn() -> K>,
}

impl<K: ContentKind> Id<K> {
    pub(crate) fn hash_body(body: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"aver:");
        hasher.update(K::PREFIX.as_bytes());
        hasher.update(b"\0");
        hasher.update(body);
        Self::from_hash(*hasher.finalize().as_bytes())
    }

    fn from_hash(bytes: [u8; 32]) -> Self {
        Self {
            bytes,
            kind: PhantomData,
        }
    }

    /// The 32 hash bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    pub fn prefix(&self) -> &'static str {
        K::PREFIX
    }
}

impl<K> Id<K> {
    fn text(&self) -> [u8; HASH_CHARS] {
        let mut text = [0u8; HASH_CHARS];
        BASE32_LOWER.encode_mut(&self.bytes, &mut text);
        text
    }
}

impl BlobId {
    /// The id of a blob with these contents.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self::hash_body(bytes)
    }
}

impl AgentId {
    /// The id of the agent holding this Ed25519 key: the hash of the raw 32-byte public key.
    pub fn of_public_key(key: &VerifyingKey) -> Self {
        Self::hash_body(key.as_bytes())
    }
}

impl<K: ContentKind> fmt::Display for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.text();
        let text = std::str::from_utf8(&text).expect("base32 is ASCII");
        write!(f, "{}_{text}", K::PREFIX)
    }
}

impl<K: ContentKind> fmt::Debug for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<K: ContentKind> FromStr for Id<K> {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        let invalid = |reason: &str| ParseError::invalid(K::PREFIX, format!("{reason}: {s:?}"));
        let rest = s
            .strip_prefix(K::PREFIX)
            .and_then(|r| r.strip_prefix('_'))
            .ok_or_else(|| invalid("wrong prefix"))?;
        if rest.len() != HASH_CHARS {
            return Err(invalid("wrong length"));
        }
        let decoded = BASE32_LOWER
            .decode(rest.as_bytes())
            .map_err(|_| invalid("not lowercase base32"))?;
        let bytes: [u8; 32] = decoded.try_into().map_err(|_| invalid("wrong length"))?;
        Ok(Self::from_hash(bytes))
    }
}

impl<K> Clone for Id<K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for Id<K> {}
impl<K> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}
impl<K> Eq for Id<K> {}
impl<K> Hash for Id<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}
// Ids order by their text, the order canonical JSON sorts strings in. Hash
// byte order differs because the base32 alphabet puts digits after letters in
// value but before them in ASCII.
impl<K> Ord for Id<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.text().cmp(&other.text())
    }
}
impl<K> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: ContentKind> Serialize for Id<K> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de, K: ContentKind> Deserialize<'de> for Id<K> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A live id of kind `K`: `<prefix>_<ULID>`.
pub struct LiveId<K> {
    ulid: Ulid,
    kind: PhantomData<fn() -> K>,
}

impl<K: LiveKind> LiveId<K> {
    pub fn from_ulid(ulid: Ulid) -> Self {
        Self {
            ulid,
            kind: PhantomData,
        }
    }

    /// A fresh id from the system clock and thread RNG.
    pub fn generate() -> Self {
        Self::from_ulid(Ulid::generate())
    }

    pub fn ulid(&self) -> Ulid {
        self.ulid
    }
}

impl<K: LiveKind> fmt::Display for LiveId<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", K::PREFIX, self.ulid)
    }
}

impl<K: LiveKind> fmt::Debug for LiveId<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<K: LiveKind> FromStr for LiveId<K> {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        let invalid = |reason: &str| ParseError::invalid(K::PREFIX, format!("{reason}: {s:?}"));
        let rest = s
            .strip_prefix(K::PREFIX)
            .and_then(|r| r.strip_prefix('_'))
            .ok_or_else(|| invalid("wrong prefix"))?;
        if rest.len() != ulid::ULID_LEN {
            return Err(invalid("wrong length"));
        }
        let ulid = Ulid::from_string(rest).map_err(|_| invalid("not a ULID"))?;
        if ulid.to_string() != rest {
            return Err(invalid("ULID not in canonical uppercase form"));
        }
        Ok(Self::from_ulid(ulid))
    }
}

impl<K> Clone for LiveId<K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for LiveId<K> {}
impl<K> PartialEq for LiveId<K> {
    fn eq(&self, other: &Self) -> bool {
        self.ulid == other.ulid
    }
}
impl<K> Eq for LiveId<K> {}
impl<K> Hash for LiveId<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ulid.hash(state);
    }
}
impl<K> Ord for LiveId<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ulid.cmp(&other.ulid)
    }
}
impl<K> PartialOrd for LiveId<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: LiveKind> Serialize for LiveId<K> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de, K: LiveKind> Deserialize<'de> for LiveId<K> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
