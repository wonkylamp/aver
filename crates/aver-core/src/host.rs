//! Host profile (ARCHITECTURE.md §1.4.3): what a native run compiles and
//! runs with. Derived, never stored in a hashed object except as a v3
//! receipt's `provenance.host_profile`.

use serde::{Deserialize, Serialize};

use crate::env::{HostArch, HostOs};
use crate::ParseError;

/// The executor profile id of native runs is this prefix and the first 16
/// hex digits of the host profile (§1.9.1).
pub const NATIVE_EXECUTOR_PREFIX: &str = "native-v0.";

/// One toolchain binary the clean `PATH` resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTool {
    /// The name a command uses, e.g. `cargo`.
    pub name: String,
    /// The real path it resolved to, symlinks followed.
    pub path: String,
    /// What it printed for `--version` (stdout and stderr).
    pub version: Vec<u8>,
}

/// 64 lowercase hex digits naming a host's OS, CPU and toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HostProfile(String);

impl HostProfile {
    /// BLAKE3 over `"aver:host\0"` and the canonical JSON of the OS, the
    /// CPU and every tool as `[name, realpath, BLAKE3 hex of its version
    /// output]`, sorted by name (and path, for a name given twice).
    pub fn of(os: HostOs, arch: HostArch, tools: &[HostTool]) -> Self {
        let mut rows: Vec<[String; 3]> = tools
            .iter()
            .map(|t| {
                [
                    t.name.clone(),
                    t.path.clone(),
                    blake3::hash(&t.version).to_hex().to_string(),
                ]
            })
            .collect();
        rows.sort();
        rows.dedup();
        let identity = serde_json::json!({ "os": os, "arch": arch, "tools": rows });
        let body = crate::canonical::to_canonical(&identity)
            .expect("the host identity holds only strings");
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"aver:host\0");
        hasher.update(&body);
        Self(hasher.finalize().to_hex().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The executor profile id runs on this host record:
    /// `native-v0.<first 16 hex digits>`.
    pub fn executor(&self) -> String {
        format!("{NATIVE_EXECUTOR_PREFIX}{}", &self.0[..16])
    }
}

impl TryFrom<String> for HostProfile {
    type Error = ParseError;
    fn try_from(s: String) -> Result<Self, ParseError> {
        s.parse()
    }
}

impl From<HostProfile> for String {
    fn from(p: HostProfile) -> Self {
        p.0
    }
}

impl std::str::FromStr for HostProfile {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, ParseError> {
        if s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            Ok(Self(s.to_owned()))
        } else {
            Err(ParseError::invalid(
                "host profile",
                format!("not 64 lowercase hex digits: {s:?}"),
            ))
        }
    }
}

/// A short, stable directory name for a host path (a repository's
/// directory, an `$AVER_HOME`): 16 hex digits of BLAKE3 over
/// `"aver:local\0"` and the path's bytes. Names host-local caches only.
pub fn local_name(path: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"aver:local\0");
    hasher.update(path);
    hasher.finalize().to_hex()[..16].to_owned()
}

impl std::fmt::Display for HostProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
