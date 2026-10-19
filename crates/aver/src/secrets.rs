//! High-confidence secret detection for imported and written files
//! (ARCHITECTURE.md §6.5). A finding names the path and pattern, never the
//! matched bytes, so reporting a secret cannot leak it.
//!
//! Binary files (a NUL byte in the first 8 KiB) are not scanned: the patterns
//! are text patterns, and a database or image that happens to hold one is a
//! false positive far more often than a credential. Findings in templates,
//! test and fixture paths, examples and Markdown documentation are
//! [`Severity::Low`]: those are where secret-handling projects keep fake keys
//! on purpose.

use std::sync::OnceLock;

use aver_core::Path;
use regex::bytes::Regex;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SecretFinding {
    pub path: String,
    pub pattern: &'static str,
    pub severity: Severity,
}

/// How likely a finding is to be a real credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// A template, a test or fixture path, an example or documentation.
    Low,
    High,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
        }
    }
}

impl std::fmt::Display for SecretFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} looks like it contains a secret ({})",
            self.path, self.pattern
        )
    }
}

const PATTERNS: &[(&str, &str)] = &[
    (
        "pem-private-key",
        r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----",
    ),
    ("aws-access-key-id", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
    ("github-token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b"),
    (
        "github-fine-grained-token",
        r"\bgithub_pat_[A-Za-z0-9_]{60,}\b",
    ),
    ("slack-token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
    ("npm-token", r"\bnpm_[A-Za-z0-9]{36}\b"),
    ("anthropic-api-key", r"\bsk-ant-[A-Za-z0-9_-]{20,}"),
    ("openai-api-key", r"\bsk-(?:proj-)?[A-Za-z0-9]{40,}\b"),
    ("stripe-live-key", r"\b[sr]k_live_[A-Za-z0-9]{20,}\b"),
];

fn compiled() -> &'static [(&'static str, Regex)] {
    static CELL: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    CELL.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|(name, re)| (*name, Regex::new(re).expect("secret pattern compiles")))
            .collect()
    })
}

fn dotenv_assignment() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r"(?m)^\s*(?:export\s+)?[A-Za-z_][A-Za-z0-9_]*\s*=\s*\S").expect("compiles")
    })
}

/// Suffixes of files meant to be committed as templates: `.env.example`,
/// `.env.test.example`, `config.sample`.
const TEMPLATE_SUFFIXES: &[&str] = &["example", "sample", "template", "dist", "defaults"];

/// Directory names whose files are tests, fixtures or examples.
const LOW_DIRS: &[&str] = &[
    "test",
    "tests",
    "__tests__",
    "spec",
    "specs",
    "fixture",
    "fixtures",
    "__fixtures__",
    "testdata",
    "test-data",
    "__mocks__",
    "mocks",
    "example",
    "examples",
];

fn file_name(path: &Path) -> &str {
    path.as_str().rsplit('/').next().unwrap_or_default()
}

fn is_template(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, suffix)| TEMPLATE_SUFFIXES.contains(&suffix))
}

/// `.env` and `.env.<anything>`, except templates meant to be committed.
fn is_dotenv(path: &Path) -> bool {
    let name = file_name(path);
    name == ".env" || (name.starts_with(".env.") && !is_template(name))
}

/// Whether a finding in `path` is [`Severity::Low`]: a template, a file
/// under a test, fixture or example directory, a test file by name, or
/// Markdown documentation.
fn severity(path: &Path) -> Severity {
    let name = file_name(path);
    let mut dirs = path.as_str().split('/');
    dirs.next_back();
    let low = is_template(name)
        || dirs.any(|d| LOW_DIRS.contains(&d))
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with(".md")
        || name.ends_with(".mdx");
    if low {
        Severity::Low
    } else {
        Severity::High
    }
}

/// Bytes inspected for a NUL to tell a binary file from text.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

/// Returns the first secret-like finding in `bytes`, if any. Binary files
/// are not scanned.
pub fn scan(path: &Path, bytes: &[u8]) -> Option<SecretFinding> {
    if is_binary(bytes) {
        return None;
    }
    let finding = |pattern| SecretFinding {
        path: path.as_str().to_owned(),
        pattern,
        severity: severity(path),
    };
    if is_dotenv(path) && dotenv_assignment().is_match(bytes) {
        return Some(finding("dotenv-file"));
    }
    compiled()
        .iter()
        .find(|(_, re)| re.is_match(bytes))
        .map(|(name, _)| finding(name))
}

/// The finding `scan` reports for pattern `name` in `path`, with the
/// severity today's rules give that path; `None` for a pattern name this
/// scanner no longer has.
pub fn finding(path: &Path, name: &str) -> Option<SecretFinding> {
    let pattern = std::iter::once("dotenv-file")
        .chain(PATTERNS.iter().map(|(n, _)| *n))
        .find(|n| *n == name)?;
    Some(SecretFinding {
        path: path.as_str().to_owned(),
        pattern,
        severity: severity(path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Path {
        s.parse().unwrap()
    }

    #[test]
    fn detects_high_confidence_patterns_without_echoing_them() {
        let key = format!("-----BEGIN RSA {}-----\nabc", "PRIVATE KEY");
        let f = scan(&p("keys/id_rsa"), key.as_bytes()).unwrap();
        assert_eq!(f.pattern, "pem-private-key");
        assert!(!f.to_string().contains("BEGIN"));
        let aws = format!("const k = '{}{}';", "AKIA", "ABCDEFGHIJKLMNOP");
        assert_eq!(
            scan(&p("src/a.ts"), aws.as_bytes()).unwrap().pattern,
            "aws-access-key-id"
        );
        let gh = format!("token={}{}", "ghp_", "a".repeat(36));
        assert_eq!(
            scan(&p("ci.yml"), gh.as_bytes()).unwrap().pattern,
            "github-token"
        );
    }

    #[test]
    fn dotenv_files_with_assignments_are_secrets_but_templates_are_not() {
        assert_eq!(
            scan(&p(".env"), b"DB_PASSWORD=hunter2\n").unwrap().pattern,
            "dotenv-file"
        );
        assert_eq!(
            scan(&p("app/.env.production"), b"X=1").unwrap().pattern,
            "dotenv-file"
        );
        assert!(scan(&p(".env.example"), b"DB_PASSWORD=changeme\n").is_none());
        assert!(scan(&p(".env.n8n.example"), b"N8N_API_KEY=changeme\n").is_none());
        assert!(scan(&p(".env"), b"# empty\n").is_none());
    }

    #[test]
    fn binary_files_are_not_scanned() {
        let mut db = b"SQLite format 3\0".to_vec();
        db.extend_from_slice(format!("{}{}", "AKIA", "ABCDEFGHIJKLMNOP").as_bytes());
        assert!(scan(&p("data/nodes.db"), &db).is_none());
    }

    #[test]
    fn fixtures_templates_and_docs_are_low_severity() {
        let key = format!("-----BEGIN RSA {}-----\nabc", "PRIVATE KEY");
        for path in [
            "tests/.env",
            "src/test/fixtures/secrets/noSecret1.ts",
            "tests/core/security/worker.test.ts",
            "README.md",
            "config/.env.azure.example",
            "examples/keys/demo.pem",
        ] {
            let bytes = if path.ends_with(".env") {
                b"X=1".to_vec()
            } else {
                key.clone().into_bytes()
            };
            assert_eq!(
                scan(&p(path), &bytes).map(|f| f.severity),
                Some(Severity::Low),
                "{path}"
            );
        }
        assert_eq!(
            scan(&p("src/cli/examples.js"), key.as_bytes()).map(|f| f.severity),
            Some(Severity::High)
        );
        assert_eq!(
            scan(&p(".env.azure-pipelines"), b"X=1").map(|f| f.severity),
            Some(Severity::High)
        );
    }

    #[test]
    fn ordinary_code_is_clean() {
        let src =
            b"export function add(a: number, b: number) { return a + b }\nconst sk = 'sk-short';";
        assert!(scan(&p("src/add.ts"), src).is_none());
    }
}
