//! Derived keys: 64 lowercase hex digits of BLAKE3 over `"aver:<domain>\0"`
//! and a canonical JSON body. They name runs and caches, are computed only
//! here, and are never content ids of stored objects.

use serde::Serialize;

/// BLAKE3 over `"aver:<domain>\0"` and the canonical JSON of `value`.
pub(crate) fn derive(domain: &str, value: &impl Serialize) -> String {
    let value = serde_json::to_value(value).expect("key bodies always serialize to JSON");
    let body = crate::canonical::to_canonical(&value)
        .expect("key bodies hold only strings, small integers and arrays");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"aver:");
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(&body);
    hasher.finalize().to_hex().to_string()
}

macro_rules! hex_key {
    ($(#[$meta:meta])* $name:ident, $label:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::ParseError;
            fn from_str(s: &str) -> Result<Self, $crate::ParseError> {
                if s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                    Ok(Self(s.to_owned()))
                } else {
                    Err($crate::ParseError::invalid(
                        $label,
                        format!("not 64 lowercase hex digits: {s:?}"),
                    ))
                }
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::ParseError;
            fn try_from(s: String) -> Result<Self, $crate::ParseError> {
                s.parse()
            }
        }

        impl From<$name> for String {
            fn from(key: $name) -> Self {
                key.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
pub(crate) use hex_key;
