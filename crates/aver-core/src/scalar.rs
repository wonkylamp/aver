//! Validated scalar types (ARCHITECTURE.md §1.1.4 and the string fields of §1).
//!
//! Each type is a string checked once on construction (`FromStr`, `TryFrom`,
//! or serde) and trusted afterwards. Ordering is byte order of the text,
//! which is the order canonical sets use.

use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

use crate::ParseError;

macro_rules! string_scalar {
    ($(#[$meta:meta])* $name:ident, $label:literal, $validate:path) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = ParseError;
            fn from_str(s: &str) -> Result<Self, ParseError> {
                $validate(s).map_err(|reason| ParseError::invalid($label, format!("{reason}: {s:?}")))?;
                Ok(Self(s.to_owned()))
            }
        }

        impl TryFrom<String> for $name {
            type Error = ParseError;
            fn try_from(s: String) -> Result<Self, ParseError> {
                $validate(&s).map_err(|reason| ParseError::invalid($label, format!("{reason}: {s:?}")))?;
                Ok(Self(s))
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&self.0, f)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::try_from(String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

const MAX_PATH_BYTES: usize = 4096;
const MAX_TEST_PATH_BYTES: usize = 1024;

fn validate_name(s: &str) -> Result<(), &'static str> {
    if s.contains('/') {
        return Err("contains '/'");
    }
    validate_segment(s)?;
    if s.len() > MAX_PATH_BYTES {
        return Err("longer than 4096 bytes");
    }
    Ok(())
}

fn validate_segment(s: &str) -> Result<(), &'static str> {
    match s {
        "" => Err("empty segment"),
        "." | ".." => Err("'.' or '..' segment"),
        _ if s.eq_ignore_ascii_case(".git") => Err("'.git' segment"),
        _ if s.contains('\0') => Err("contains NUL"),
        _ => Ok(()),
    }
}

fn validate_path(s: &str) -> Result<(), &'static str> {
    if s.len() > MAX_PATH_BYTES {
        return Err("longer than 4096 bytes");
    }
    s.split('/').try_for_each(validate_segment)
}

string_scalar!(
    /// Repo-relative, `/`-separated path. No leading or trailing `/`, no empty,
    /// `.`, `..` or `.git` segments, no NUL, at most 4096 bytes.
    Path,
    "path",
    validate_path
);

impl Path {
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

string_scalar!(
    /// One path segment, with the same rules as [`Path`].
    Name,
    "name",
    validate_name
);

fn is_check_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    matches!(bytes.first(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

fn validate_check_id(s: &str) -> Result<(), &'static str> {
    if is_check_id(s) {
        Ok(())
    } else {
        Err("must match [a-z0-9][a-z0-9._-]{0,63}")
    }
}

string_scalar!(
    /// `[a-z0-9][a-z0-9._-]{0,63}`
    CheckId,
    "check id",
    validate_check_id
);

fn validate_test_check_id(s: &str) -> Result<(), &'static str> {
    let (check, test) = s.split_once("::").ok_or("missing '::'")?;
    validate_check_id(check)?;
    if test.is_empty() {
        return Err("empty test path");
    }
    if test.len() > MAX_TEST_PATH_BYTES {
        return Err("test path longer than 1024 bytes");
    }
    Ok(())
}

string_scalar!(
    /// `<CheckId>::<test path>`, where the test path is the JUnit suite chain
    /// and test name joined by ` > `, at most 1024 bytes.
    TestCheckId,
    "test check id",
    validate_test_check_id
);

impl TestCheckId {
    pub fn new(check: &CheckId, test_path: &str) -> Result<Self, ParseError> {
        format!("{check}::{test_path}").parse()
    }

    pub fn check(&self) -> CheckId {
        let (check, _) = self.0.split_once("::").expect("validated on construction");
        CheckId(check.to_owned())
    }

    pub fn test_path(&self) -> &str {
        let (_, test) = self.0.split_once("::").expect("validated on construction");
        test
    }
}

fn validate_entry_id(s: &str) -> Result<(), &'static str> {
    if s.contains("::") {
        validate_test_check_id(s)
    } else {
        validate_check_id(s)
    }
}

string_scalar!(
    /// The id of a receipt check entry: a [`CheckId`] for a command-level entry
    /// or a [`TestCheckId`] for a per-test entry.
    EntryId,
    "receipt entry id",
    validate_entry_id
);

impl From<CheckId> for EntryId {
    fn from(id: CheckId) -> Self {
        Self(id.0)
    }
}

impl From<TestCheckId> for EntryId {
    fn from(id: TestCheckId) -> Self {
        Self(id.0)
    }
}

fn validate_criterion_id(s: &str) -> Result<(), &'static str> {
    let bytes = s.as_bytes();
    let ok = matches!(bytes.first(), Some(b) if b.is_ascii_alphanumeric())
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err("must match [A-Za-z0-9][A-Za-z0-9._-]{0,63}")
    }
}

string_scalar!(
    /// `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`
    CriterionId,
    "criterion id",
    validate_criterion_id
);

fn validate_hex(s: &str, len: usize) -> Result<(), &'static str> {
    if s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err("wrong length or not lowercase hex")
    }
}

fn validate_digest(s: &str) -> Result<(), &'static str> {
    let hex = s
        .strip_prefix("sha256:")
        .ok_or("missing 'sha256:' prefix")?;
    validate_hex(hex, 64)
}

string_scalar!(
    /// OCI image digest, `sha256:<64 lowercase hex>`.
    Digest,
    "digest",
    validate_digest
);

fn validate_git_sha(s: &str) -> Result<(), &'static str> {
    validate_hex(s, 40)
}

string_scalar!(
    /// A Git object name, 40 lowercase hex characters.
    GitSha,
    "git sha",
    validate_git_sha
);

const MAX_REF_BYTES: usize = 255;

fn validate_ref_name(s: &str) -> Result<(), &'static str> {
    if s.len() > MAX_REF_BYTES {
        return Err("longer than 255 bytes");
    }
    for segment in s.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("empty, '.' or '..' segment");
        }
        let ok = segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'));
        if !ok {
            return Err("segments may contain only [A-Za-z0-9._:-]");
        }
    }
    Ok(())
}

string_scalar!(
    /// A ref name such as `trunk` or `ledger/ref:trunk`: `/`-separated segments
    /// of `[A-Za-z0-9._:-]`, no empty, `.` or `..` segments, at most 255 bytes.
    RefName,
    "ref name",
    validate_ref_name
);

const MAX_GLOB_BYTES: usize = 1024;

fn validate_path_glob(s: &str) -> Result<(), &'static str> {
    if s.len() > MAX_GLOB_BYTES {
        return Err("longer than 1024 bytes");
    }
    for segment in s.split('/') {
        match segment {
            "" => return Err("empty segment"),
            "." | ".." => return Err("'.' or '..' segment"),
            "**" => {}
            _ if segment.contains("**") => return Err("'**' must be a whole segment"),
            _ if segment.contains('\0') => return Err("contains NUL"),
            _ if segment.contains('\\') => return Err("contains '\\'"),
            _ => {}
        }
    }
    Ok(())
}

string_scalar!(
    /// A repo-relative path pattern (ARCHITECTURE.md §1.7): `/`-separated
    /// segments, no leading or trailing `/`, no empty, `.` or `..` segments, no
    /// NUL or `\`, at most 1024 bytes. In a segment `*` matches any run of
    /// characters and `?` exactly one; a whole segment `**` matches any number
    /// of segments, including none. Every other character is literal.
    PathGlob,
    "path glob",
    validate_path_glob
);

impl PathGlob {
    /// Whether `path` (repo-relative, `/`-separated) is the pattern or lies
    /// under a directory the pattern names, so `test` and `test/**` both
    /// cover `test/unit/a.js`.
    pub fn covers(&self, path: &str) -> bool {
        let pattern: Vec<&str> = self.0.split('/').collect();
        let path: Vec<&str> = path.split('/').collect();
        // matched[j]: the pattern so far matches the first j path segments.
        let mut matched = vec![false; path.len() + 1];
        matched[0] = true;
        for seg in &pattern {
            let mut next = vec![false; path.len() + 1];
            for j in 0..=path.len() {
                next[j] = if *seg == "**" {
                    matched[j] || (j > 0 && next[j - 1])
                } else {
                    j > 0 && matched[j - 1] && segment_matches(seg, path[j - 1])
                };
            }
            matched = next;
        }
        matched[1..].iter().any(|m| *m)
    }

    /// Whether some path is covered by both patterns (leases conflict on
    /// this, §1.8.4). Each pattern stands for itself plus everything under
    /// it, so the check runs on both with a trailing `**` and asks whether
    /// one sequence of segments matches both.
    pub fn overlaps(&self, other: &PathGlob) -> bool {
        let a: Vec<&str> = self.0.split('/').chain(["**"]).collect();
        let b: Vec<&str> = other.0.split('/').chain(["**"]).collect();
        intersects(
            &a,
            &b,
            |x, y| *x == "**" || *y == "**" || segments_intersect(x, y),
            |x| *x == "**",
        )
    }
}

/// Whether two wildcard sequences share a word: `star(x)` items match any
/// run of symbols (including none), others one symbol, and `meet(x, y)`
/// says whether two single-symbol items can match the same symbol. A
/// breadth-first walk over pairs of positions (the product automaton).
fn intersects<T>(
    a: &[T],
    b: &[T],
    meet: impl Fn(&T, &T) -> bool,
    star: impl Fn(&T) -> bool,
) -> bool {
    let width = b.len() + 1;
    let mut seen = vec![false; (a.len() + 1) * width];
    let mut queue = vec![(0usize, 0usize)];
    while let Some((i, j)) = queue.pop() {
        if seen[i * width + j] {
            continue;
        }
        seen[i * width + j] = true;
        if i == a.len() && j == b.len() {
            return true;
        }
        if i < a.len() && star(&a[i]) {
            queue.push((i + 1, j));
        }
        if j < b.len() && star(&b[j]) {
            queue.push((i, j + 1));
        }
        if i < a.len() && j < b.len() && meet(&a[i], &b[j]) {
            let ni = if star(&a[i]) { i } else { i + 1 };
            let nj = if star(&b[j]) { j } else { j + 1 };
            if (ni, nj) != (i, j) {
                queue.push((ni, nj));
            }
        }
    }
    false
}

/// Whether two segment patterns match a common segment.
fn segments_intersect(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    intersects(
        &a,
        &b,
        |x, y| matches!(x, '*' | '?') || matches!(y, '*' | '?') || x == y,
        |c| *c == '*',
    )
}

/// `*` and `?` wildcard match of one segment, by character.
fn segment_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let mut matched = vec![false; text.len() + 1];
    matched[0] = true;
    for p in &pattern {
        let mut next = vec![false; text.len() + 1];
        for j in 0..=text.len() {
            next[j] = match p {
                '*' => matched[j] || (j > 0 && next[j - 1]),
                '?' => j > 0 && matched[j - 1],
                c => j > 0 && matched[j - 1] && text[j - 1] == *c,
            };
        }
        matched = next;
    }
    matched[text.len()]
}

static SECRET_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(token|secret|password|passwd|api[_-]?key|private|credential|auth)")
        .expect("valid regex")
});

fn validate_env_var_name(s: &str) -> Result<(), &'static str> {
    let bytes = s.as_bytes();
    let ok = matches!(bytes.first(), Some(b) if b.is_ascii_alphabetic() || *b == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_');
    if !ok {
        return Err("must match [A-Za-z_][A-Za-z0-9_]*");
    }
    if SECRET_VAR.is_match(s) {
        return Err("looks like a secret; secrets never enter environments");
    }
    Ok(())
}

string_scalar!(
    /// A non-secret environment variable name, `[A-Za-z_][A-Za-z0-9_]*`. Names
    /// that look like secrets (token, password, api key, ...) are rejected.
    EnvVarName,
    "environment variable name",
    validate_env_var_name
);

const REPORT_DIR: &str = "/aver/out/";

fn validate_report_path(s: &str) -> Result<(), &'static str> {
    let rest = s
        .strip_prefix(REPORT_DIR)
        .ok_or("must start with /aver/out/")?;
    validate_path(rest)
}

string_scalar!(
    /// Absolute path of a check's report inside the sandbox, under `/aver/out/`.
    ReportPath,
    "report path",
    validate_report_path
);

/// Most bytes a CI receipt's subject may have.
pub const MAX_SUBJECT_BYTES: usize = 256;

fn validate_subject(s: &str) -> Result<(), &'static str> {
    if s.is_empty() || s.len() > MAX_SUBJECT_BYTES {
        return Err("must be 1 to 256 bytes");
    }
    if s.chars().any(char::is_control) {
        return Err("must not contain control characters");
    }
    Ok(())
}

string_scalar!(
    /// A consumer's label for a CI receipt (`aver ci --subject`): 1 to 256
    /// bytes of UTF-8 without control characters.
    Subject,
    "subject",
    validate_subject
);

/// Most bytes a contract's shell line may have.
pub const MAX_SHELL_LINE_BYTES: usize = 4096;

fn validate_shell_line(s: &str) -> Result<(), &'static str> {
    if s.trim().is_empty() || s.len() > MAX_SHELL_LINE_BYTES {
        return Err("must be 1 to 4096 bytes, not only whitespace");
    }
    if s.contains('\0') {
        return Err("contains NUL");
    }
    Ok(())
}

string_scalar!(
    /// A command a contract runs with `sh -c`, as written: 1 to 4096 bytes,
    /// not only whitespace, no NUL.
    ShellLine,
    "shell line",
    validate_shell_line
);

/// A UTF-8 string of at most `MAX` bytes.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Text<const MAX: usize>(String);

impl<const MAX: usize> Text<MAX> {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Truncates `s` to at most `MAX` bytes on a character boundary.
    pub fn truncated(s: &str) -> Self {
        let mut end = s.len().min(MAX);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        Self(s[..end].to_owned())
    }
}

impl<const MAX: usize> TryFrom<String> for Text<MAX> {
    type Error = ParseError;
    fn try_from(s: String) -> Result<Self, ParseError> {
        if s.len() > MAX {
            return Err(ParseError::invalid(
                "text",
                format!("{} bytes exceeds the {MAX}-byte limit", s.len()),
            ));
        }
        Ok(Self(s))
    }
}

impl<const MAX: usize> FromStr for Text<MAX> {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, ParseError> {
        Self::try_from(s.to_owned())
    }
}

impl<const MAX: usize> fmt::Display for Text<MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<const MAX: usize> fmt::Debug for Text<MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<const MAX: usize> Serialize for Text<MAX> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for Text<MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::try_from(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

const TIMESTAMP_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

/// RFC 3339 UTC instant with millisecond precision, exactly
/// `YYYY-MM-DDTHH:MM:SS.sssZ`, years 0000 through 9999.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    unix_millis: i64,
}

impl Timestamp {
    pub fn from_unix_millis(unix_millis: i64) -> Result<Self, ParseError> {
        let out_of_range = || {
            ParseError::invalid(
                "timestamp",
                format!("{unix_millis} ms is outside years 0000-9999"),
            )
        };
        let dt = OffsetDateTime::from_unix_timestamp_nanos(i128::from(unix_millis) * 1_000_000)
            .map_err(|_| out_of_range())?;
        if !(0..=9999).contains(&dt.year()) {
            return Err(out_of_range());
        }
        Ok(Self { unix_millis })
    }

    pub fn now() -> Self {
        let millis = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
        Self::from_unix_millis(millis as i64).expect("the system clock is within years 0000-9999")
    }

    pub fn unix_millis(&self) -> i64 {
        self.unix_millis
    }

    fn datetime(&self) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(self.unix_millis) * 1_000_000)
            .expect("range checked on construction")
    }
}

impl FromStr for Timestamp {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, ParseError> {
        let invalid = || {
            ParseError::invalid(
                "timestamp",
                format!("expected YYYY-MM-DDTHH:MM:SS.sssZ: {s:?}"),
            )
        };
        let dt = time::PrimitiveDateTime::parse(s, TIMESTAMP_FORMAT)
            .map_err(|_| invalid())?
            .assume_utc();
        let ts = Self::from_unix_millis((dt.unix_timestamp_nanos() / 1_000_000) as i64)?;
        if ts.to_string() != s {
            return Err(invalid());
        }
        Ok(ts)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self
            .datetime()
            .format(TIMESTAMP_FORMAT)
            .map_err(|_| fmt::Error)?;
        f.write_str(&text)
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
