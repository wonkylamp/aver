//! The [`Object`] contract shared by every hashed object type.
//!
//! An object serializes to canonical JSON, and its id is the domain-separated
//! hash of those bytes. [`Object::from_canonical`] is the strict parser: it
//! accepts exactly the bytes that [`Object::canonical_bytes`] would produce,
//! so `parse(bytes).id()` always equals the hash of `bytes`.
//!
//! Every object struct carries `schema: Schema<Self>`, a zero-sized field that
//! serializes as the version string (`"aver.tree/v0"`) and rejects any other
//! value on parse.

use std::fmt;
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::canonical;
use crate::id::{ContentKind, Id};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("invalid {kind}: {reason}")]
    Invalid { kind: &'static str, reason: String },
    #[error("malformed object: {0}")]
    Json(String),
    #[error("object bytes are not canonical JSON")]
    NonCanonical,
}

impl ParseError {
    pub(crate) fn invalid(kind: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            kind,
            reason: reason.into(),
        }
    }
}

/// A type with a published JSON schema version string.
pub trait Versioned {
    const SCHEMA: &'static str;
}

/// The `schema` field of an object of type `T`.
pub struct Schema<T>(PhantomData<fn() -> T>);

impl<T> Schema<T> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> Default for Schema<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for Schema<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Schema<T> {}
impl<T> PartialEq for Schema<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl<T> Eq for Schema<T> {}

impl<T: Versioned> fmt::Debug for Schema<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(T::SCHEMA)
    }
}

impl<T: Versioned> Serialize for Schema<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(T::SCHEMA)
    }
}

impl<'de, T: Versioned> Deserialize<'de> for Schema<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s != T::SCHEMA {
            return Err(serde::de::Error::custom(format!(
                "unsupported schema {s:?}, expected {:?}",
                T::SCHEMA
            )));
        }
        Ok(Self::new())
    }
}

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// A content-addressed object stored as canonical JSON. Implemented only by
/// the object types in this crate, so ids are computed in one place.
pub trait Object: Serialize + DeserializeOwned + sealed::Sealed {
    type Kind: ContentKind;

    /// The canonical JSON encoding hashed by [`Object::id`].
    fn canonical_bytes(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("object types always serialize to JSON");
        // Object types hold only u32/i32 integers, ASCII keys and no floats, so
        // canonicalization cannot fail.
        canonical::to_canonical(&value).expect("object types are canonical by construction")
    }

    fn id(&self) -> Id<Self::Kind> {
        Id::hash_body(&self.canonical_bytes())
    }

    /// Strictly parses canonical bytes: unknown fields, wrong types, floats,
    /// unsorted sets, a mismatched `schema` and any non-canonical encoding
    /// (whitespace, key order, explicit nulls for omitted fields) are errors.
    fn from_canonical(bytes: &[u8]) -> Result<Self, ParseError> {
        let object: Self =
            serde_json::from_slice(bytes).map_err(|e| ParseError::Json(e.to_string()))?;
        if object.canonical_bytes() != bytes {
            return Err(ParseError::NonCanonical);
        }
        Ok(object)
    }
}

/// Requires a nullable field to be present, so `null` and absence are not two
/// encodings of the same value. Use as `#[serde(deserialize_with = "nullable")]`.
pub(crate) fn nullable<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}

macro_rules! impl_object {
    ($ty:ty, $kind:ty, $schema:literal) => {
        impl $crate::object::Versioned for $ty {
            const SCHEMA: &'static str = $schema;
        }
        impl $crate::object::sealed::Sealed for $ty {}
        impl $crate::object::Object for $ty {
            type Kind = $kind;
        }
    };
}
pub(crate) use impl_object;
