//! Set-typed arrays (ARCHITECTURE.md §1.1.1).
//!
//! A [`Set`] is a `Vec` whose items are sorted by key and unique by key. It
//! can only be built through [`Set::new`], which sorts, or by deserializing,
//! which rejects unsorted or duplicate input. Either way the serialized form
//! is the one canonical order.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ParseError;

/// An item that can live in a [`Set`], ordered and deduplicated by `key()`.
pub trait SetItem {
    type Key: Ord + fmt::Debug + ?Sized;
    fn key(&self) -> &Self::Key;
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Set<T>(Vec<T>);

impl<T: SetItem + PartialEq> Set<T> {
    /// Sorts by key and drops exact duplicates. Two different items with the
    /// same key are an error.
    pub fn new(mut items: Vec<T>) -> Result<Self, ParseError> {
        items.sort_by(|a, b| a.key().cmp(b.key()));
        items.dedup();
        if let Some(pair) = items.windows(2).find(|w| w[0].key() == w[1].key()) {
            return Err(ParseError::invalid(
                "set",
                format!("two different items share the key {:?}", pair[0].key()),
            ));
        }
        Ok(Self(items))
    }
}

impl<T: SetItem> Set<T> {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn get(&self, key: &T::Key) -> Option<&T> {
        self.0
            .binary_search_by(|item| item.key().cmp(key))
            .ok()
            .map(|i| &self.0[i])
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }

    fn check_sorted(items: Vec<T>) -> Result<Self, ParseError> {
        if let Some(pair) = items.windows(2).find(|w| w[0].key() >= w[1].key()) {
            return Err(ParseError::invalid(
                "set",
                format!(
                    "items must be sorted and unique by key; {:?} is followed by {:?}",
                    pair[0].key(),
                    pair[1].key()
                ),
            ));
        }
        Ok(Self(items))
    }
}

impl<T: SetItem> Default for Set<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<'a, T> IntoIterator for &'a Set<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: fmt::Debug> fmt::Debug for Set<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(&self.0).finish()
    }
}

impl<T: Serialize> Serialize for Set<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: SetItem + Deserialize<'de>> Deserialize<'de> for Set<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::check_sorted(Vec::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

macro_rules! self_keyed {
    ($($ty:ty),*) => {$(
        impl SetItem for $ty {
            type Key = str;
            fn key(&self) -> &str {
                self.as_str()
            }
        }
    )*};
}

self_keyed!(
    crate::CriterionId,
    crate::CheckId,
    crate::PathGlob,
    crate::Path,
    crate::TestCheckId,
    crate::EntryId
);
