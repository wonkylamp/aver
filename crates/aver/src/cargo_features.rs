//! Rust code no check compiles (WP6, RUST-3): a head change inside code
//! under `#[cfg(feature = "x")]` that no Cargo check covering the file
//! builds with feature `x` is uncovered (§1.9.3), so the receipt says
//! `UNPROVEN`, never `PASS`, for a change no test could have run.
//!
//! A check enables what its command line says (`--all-features`,
//! `--features`/`-F`, `--no-default-features`), the package's `default`
//! features, the features any manifest of the base asks of the package
//! through a dependency on it (a workspace member's `dev-dependencies`), and
//! what those enable in turn through the package's `[features]` table. A
//! check that does not run Cargo is not judged here: what it builds is its
//! own business, and path coverage already decides it. Gates are read by
//! [`crate::inline_tests::feature_gates`]; a file Aver cannot read, or a
//! manifest it cannot parse, counts as compiled.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use aver_core::{BlobId, CheckDef, Environment};
use serde::{Deserialize, Serialize};

use crate::inline_tests::{feature_gates, Gate};
use crate::workspace::{ChangedFile, FileEntry};

/// A changed file with code behind features no check enables, the package
/// the features belong to, and the features a check would need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unbuilt {
    pub path: String,
    pub package: String,
    pub features: Vec<String>,
}

impl Unbuilt {
    /// The command a check would run to build the code, in Cargo's syntax.
    pub fn command(&self) -> String {
        let features: Vec<String> = self
            .features
            .iter()
            .map(|f| format!("{}/{f}", self.package))
            .collect();
        format!("cargo test --workspace --features {}", features.join(","))
    }

    /// What it changes and how to cover it, without the path.
    pub fn remedy(&self, code: impl Fn(&str) -> String) -> String {
        let features: Vec<String> = self.features.iter().map(|f| code(f)).collect();
        format!(
            "changes code behind {} {} of {}, which no check enables; add a check running {} to aver.toml",
            if features.len() == 1 { "feature" } else { "features" },
            features.join(", "),
            code(&self.package),
            code(&self.command()),
        )
    }
}

/// The changed `.rs` files among `changed` with a change inside a feature
/// gate that no Cargo check of `checks` covering the file enables. `head`
/// and `base` list the two trees' files (the head's declares the modules,
/// the base's manifests decide the features); `read` returns a blob's
/// bytes.
pub fn uncompiled<E>(
    env: &Environment,
    checks: &[&CheckDef],
    changed: &[ChangedFile],
    head: impl FnOnce() -> Result<Vec<(String, FileEntry)>, E>,
    base: impl FnOnce() -> Result<Vec<(String, FileEntry)>, E>,
    mut read: impl FnMut(&BlobId) -> Result<Vec<u8>, E>,
) -> Result<Vec<Unbuilt>, E> {
    let mut text = |f: &Option<FileEntry>| -> Result<Option<String>, E> {
        Ok(match f {
            Some(f) => String::from_utf8(read(&f.blob)?).ok(),
            None => None,
        })
    };
    // Per file: the gates its changes fall in, from either side.
    let mut hit: Vec<(String, Vec<Gate>)> = Vec::new();
    for (path, base, head) in changed {
        if !path.ends_with(".rs") {
            continue;
        }
        let (b, h) = (text(base)?, text(head)?);
        let (b, h) = (b.unwrap_or_default(), h.unwrap_or_default());
        let (old, new) = changed_ranges(&b, &h);
        let mut gates: Vec<Gate> = Vec::new();
        for (src, ranges) in [(&b, old), (&h, new)] {
            if ranges.is_empty() {
                continue;
            }
            let Some(all) = feature_gates(src) else {
                continue;
            };
            gates.extend(
                all.into_iter()
                    .filter(|g| ranges.iter().any(|r| overlaps(r, &g.range))),
            );
        }
        hit.push((path.clone(), gates));
    }
    if hit.is_empty() {
        return Ok(Vec::new());
    }
    let files = head()?;
    let by_path: BTreeMap<&str, &FileEntry> = files.iter().map(|(p, f)| (p.as_str(), f)).collect();
    let mut file_text = |path: &str| -> Result<Option<String>, E> {
        match by_path.get(path) {
            Some(f) => Ok(String::from_utf8(read(&f.blob)?).ok()),
            None => Ok(None),
        }
    };
    // A file whose module is declared under a gate: the parent's `mod`.
    for (path, gates) in &mut hit {
        let mut child = path.clone();
        for _ in 0..8 {
            let Some((parent, name)) = parent_module(&child, |p| by_path.contains_key(p)) else {
                break;
            };
            if let Some(src) = file_text(&parent)? {
                if let Some(all) = feature_gates(&src) {
                    gates.extend(
                        all.into_iter()
                            .filter(|g| g.module.as_deref() == Some(name.as_str())),
                    );
                }
            }
            child = parent;
        }
    }
    if hit.iter().all(|(_, gates)| gates.is_empty()) {
        return Ok(Vec::new());
    }
    let manifests: Vec<(String, toml::Table)> = base()?
        .iter()
        .filter(|(p, _)| p == "Cargo.toml" || p.ends_with("/Cargo.toml"))
        .filter_map(|(p, f)| {
            let bytes = read(&f.blob).ok()?;
            let table = std::str::from_utf8(&bytes)
                .ok()?
                .parse::<toml::Table>()
                .ok()?;
            Some((p.clone(), table))
        })
        .collect();
    let mut out = Vec::new();
    for (path, gates) in hit {
        if gates.is_empty() {
            continue;
        }
        let covering: Vec<&&CheckDef> = checks
            .iter()
            .filter(|c| crate::coverage::covers(env, c, &path, false))
            .collect();
        if covering.is_empty() || covering.iter().any(|c| !runs_cargo(c)) {
            continue;
        }
        let Some((dir, manifest)) = package_of(&path, &manifests) else {
            continue;
        };
        let builds = |on: &Option<BTreeSet<String>>, g: &Gate| match on {
            None => true,
            Some(on) if g.any => g.features.iter().any(|f| on.contains(f)),
            Some(on) => g.features.iter().all(|f| on.contains(f)),
        };
        let per_check: Vec<Option<BTreeSet<String>>> = covering
            .iter()
            .map(|c| enabled(c, dir, manifest, &manifests))
            .collect();
        if per_check
            .iter()
            .any(|on| gates.iter().all(|g| builds(on, g)))
        {
            continue;
        }
        // What the first covering check lacks: every feature of an `all`
        // gate it misses, the first of an `any` gate.
        let on = per_check.first().cloned().flatten().unwrap_or_default();
        let mut features: BTreeSet<String> = BTreeSet::new();
        for g in gates.iter().filter(|g| !builds(&Some(on.clone()), g)) {
            if g.any {
                features.extend(g.features.first().cloned());
            } else {
                features.extend(g.features.iter().filter(|f| !on.contains(*f)).cloned());
            }
        }
        let package = manifest
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_owned();
        out.push(Unbuilt {
            path,
            package,
            features: features.into_iter().collect(),
        });
    }
    Ok(out)
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The byte ranges of the lines that differ: the base's (deleted or
/// replaced) and the head's (added or replacing).
fn changed_ranges(base: &str, head: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let starts = |s: &str| -> Vec<usize> {
        let mut v = vec![0];
        let mut at = 0;
        for line in s.split_inclusive('\n') {
            at += line.len();
            v.push(at);
        }
        v
    };
    let (bs, hs) = (starts(base), starts(head));
    let byte = |starts: &[usize], lines: Range<usize>| {
        starts[lines.start.min(starts.len() - 1)]..starts[lines.end.min(starts.len() - 1)]
    };
    let diff = similar::TextDiff::from_lines(base, head);
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for op in diff.ops() {
        if op.tag() == similar::DiffTag::Equal {
            continue;
        }
        let (o, n) = (op.old_range(), op.new_range());
        if !o.is_empty() {
            old.push(byte(&bs, o));
        }
        if !n.is_empty() {
            new.push(byte(&hs, n));
        }
    }
    (old, new)
}

/// The file that declares the module `path` is, and the module's name:
/// `src/a/b.rs` and `src/a/b/mod.rs` are declared in `src/a.rs` or
/// `src/a/mod.rs` (or a crate root beside them), as `b`.
fn parent_module(path: &str, exists: impl Fn(&str) -> bool) -> Option<(String, String)> {
    let (dir, file) = path.rsplit_once('/')?;
    let stem = file.strip_suffix(".rs")?;
    if matches!(stem, "lib" | "main") {
        return None;
    }
    let (dir, name) = if stem == "mod" {
        let (up, name) = dir.rsplit_once('/')?;
        (up.to_owned(), name.to_owned())
    } else {
        (dir.to_owned(), stem.to_owned())
    };
    let candidates = [
        format!("{dir}/mod.rs"),
        format!("{dir}/lib.rs"),
        format!("{dir}/main.rs"),
        format!("{dir}.rs"),
    ];
    candidates
        .into_iter()
        .find(|c| c != path && exists(c))
        .map(|c| (c, name))
}

/// The package a file belongs to: the nearest manifest above it with a
/// `[package]` table, and its directory.
fn package_of<'a>(
    path: &str,
    manifests: &'a [(String, toml::Table)],
) -> Option<(&'a str, &'a toml::Table)> {
    manifests
        .iter()
        .filter(|(p, t)| {
            let dir = p.strip_suffix("Cargo.toml").unwrap_or("");
            t.contains_key("package") && path.starts_with(dir)
        })
        .max_by_key(|(p, _)| p.len())
        .map(|(p, t)| (p.strip_suffix("/Cargo.toml").unwrap_or(""), t))
}

/// Whether a check runs Cargo.
fn runs_cargo(check: &CheckDef) -> bool {
    check
        .argv
        .iter()
        .any(|a| a.split_whitespace().any(|w| w == "cargo"))
}

/// The features of the package in `dir` that `check` builds it with, or
/// `None` for all of them.
fn enabled(
    check: &CheckDef,
    dir: &str,
    manifest: &toml::Table,
    manifests: &[(String, toml::Table)],
) -> Option<BTreeSet<String>> {
    let words: Vec<String> = check
        .argv
        .iter()
        .flat_map(|a| a.split_whitespace())
        .map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_owned())
        .collect();
    if words.iter().any(|w| w == "--all-features") {
        return None;
    }
    let name = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");
    let mut on: BTreeSet<String> = BTreeSet::new();
    if !words.iter().any(|w| w == "--no-default-features") {
        on.insert("default".into());
    }
    let mut it = words.iter().peekable();
    while let Some(w) = it.next() {
        let list = if w == "--features" || w == "-F" {
            // `--features 'a b'` arrives split into words; take them all up
            // to the next flag.
            let mut list = Vec::new();
            while let Some(next) = it.peek() {
                if next.starts_with('-') || next.contains(['>', '<', '&', '|', ';']) {
                    break;
                }
                list.push(it.next().expect("peeked").clone());
            }
            list
        } else if let Some(v) = w.strip_prefix("--features=") {
            vec![v.to_owned()]
        } else {
            continue;
        };
        for item in list.iter().flat_map(|l| l.split(',')) {
            let feature = match item.split_once('/') {
                Some((pkg, f)) if pkg == name => f,
                Some(_) => continue,
                None => item,
            };
            if !feature.is_empty() {
                on.insert(feature.to_owned());
            }
        }
    }
    // What other manifests ask of this package through a dependency on it.
    for (_, table) in manifests {
        for deps in dependency_tables(table) {
            for (key, spec) in deps {
                let Some(spec) = spec.as_table() else {
                    continue;
                };
                let package = spec.get("package").and_then(|p| p.as_str()).unwrap_or(key);
                let path_ok = spec.get("path").and_then(|p| p.as_str()).is_none_or(|p| {
                    p.trim_end_matches('/')
                        .ends_with(dir.rsplit('/').next().unwrap_or(dir))
                });
                if package != name || !path_ok {
                    continue;
                }
                if spec.get("default-features").and_then(|d| d.as_bool()) != Some(false) {
                    on.insert("default".into());
                }
                for f in spec
                    .get("features")
                    .and_then(|f| f.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str())
                {
                    on.insert(f.to_owned());
                }
            }
        }
    }
    // Close over the package's own `[features]` table.
    let table = manifest.get("features").and_then(|f| f.as_table());
    let mut todo: Vec<String> = on.iter().cloned().collect();
    while let Some(f) = todo.pop() {
        let Some(implied) = table.and_then(|t| t.get(&f)).and_then(|v| v.as_array()) else {
            continue;
        };
        for i in implied.iter().filter_map(|v| v.as_str()) {
            if i.starts_with("dep:") || i.contains('/') {
                continue;
            }
            if on.insert(i.to_owned()) {
                todo.push(i.to_owned());
            }
        }
    }
    Some(on)
}

/// A manifest's dependency tables: `[dependencies]`, `[dev-dependencies]`,
/// `[build-dependencies]`, their `[target.*]` forms and
/// `[workspace.dependencies]`.
fn dependency_tables(table: &toml::Table) -> Vec<&toml::Table> {
    const KINDS: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];
    let mut out: Vec<&toml::Table> = KINDS
        .iter()
        .filter_map(|k| table.get(*k).and_then(|v| v.as_table()))
        .collect();
    if let Some(targets) = table.get("target").and_then(|t| t.as_table()) {
        for target in targets.values().filter_map(|t| t.as_table()) {
            out.extend(
                KINDS
                    .iter()
                    .filter_map(|k| target.get(*k).and_then(|v| v.as_table())),
            );
        }
    }
    if let Some(ws) = table
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table())
    {
        out.push(ws);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = "pub fn plain() -> u8 {\n    1\n}\n\n#[cfg(feature = \"env\")]\npub fn env() -> u8 {\n    2\n}\n\n#[cfg(not(feature = \"env\"))]\npub fn no_env() -> u8 {\n    3\n}\n\n#[cfg(feature = \"env\")]\nmod from_env;\n";

    #[test]
    fn crit_rust_feature_gated_changes_inside_gates_are_found() {
        let head = LIB.replace("    2\n", "    20\n");
        let (old, new) = changed_ranges(LIB, &head);
        let gates = feature_gates(&head).unwrap();
        let hit: Vec<&Gate> = gates
            .iter()
            .filter(|g| new.iter().chain(&old).any(|r| overlaps(r, &g.range)))
            .collect();
        assert_eq!(hit.len(), 1, "{gates:?}");
        assert_eq!(hit[0].features, vec!["env".to_owned()]);
        // `not(feature)` builds by default; the plain function is no gate.
        let head = LIB
            .replace("    3\n", "    30\n")
            .replace("    1\n", "    10\n");
        let (_, new) = changed_ranges(LIB, &head);
        assert!(!gates
            .iter()
            .any(|g| new.iter().any(|r| overlaps(r, &g.range))));
        assert_eq!(
            gates
                .iter()
                .filter_map(|g| g.module.as_deref())
                .collect::<Vec<_>>(),
            vec!["from_env"]
        );
    }

    #[test]
    fn crit_rust_feature_gated_enabled_features_come_from_the_command_and_manifests() {
        let manifest: toml::Table = "[package]\nname = \"clap_builder\"\n\n[features]\ndefault = [\"std\"]\nstd = []\nenv = []\nfull = [\"env\", \"dep:x\"]\n".parse().unwrap();
        let check = |cmd: &str| {
            CheckDef::new(
                "cargo".parse().unwrap(),
                vec!["sh".into(), "-c".into(), cmd.into()],
            )
        };
        let only = vec![("clap_builder/Cargo.toml".to_owned(), manifest.clone())];
        let on = enabled(
            &check("cargo test --workspace >o.txt"),
            "clap_builder",
            &manifest,
            &only,
        )
        .unwrap();
        assert!(on.contains("std") && !on.contains("env"), "{on:?}");
        let on = enabled(
            &check("cargo test --features 'full' >o"),
            "clap_builder",
            &manifest,
            &only,
        )
        .unwrap();
        assert!(on.contains("env"), "{on:?}");
        assert_eq!(
            enabled(
                &check("cargo test --all-features"),
                "clap_builder",
                &manifest,
                &only
            ),
            None
        );
        let on = enabled(
            &check("cargo test --no-default-features -F env"),
            "clap_builder",
            &manifest,
            &only,
        )
        .unwrap();
        assert!(on.contains("env") && !on.contains("std"), "{on:?}");
        // A member's dev-dependency on the package with a feature.
        let member: toml::Table = "[package]\nname = \"clap\"\n\n[dev-dependencies]\nclap_builder = { path = \"../clap_builder\", features = [\"env\"] }\n".parse().unwrap();
        let both = vec![only[0].clone(), ("clap/Cargo.toml".to_owned(), member)];
        let on = enabled(
            &check("cargo test --workspace"),
            "clap_builder",
            &manifest,
            &both,
        )
        .unwrap();
        assert!(on.contains("env"), "{on:?}");
    }

    #[test]
    fn parent_modules_are_found() {
        let have = ["src/lib.rs", "src/parser/mod.rs", "src/parser/env.rs"];
        let exists = |p: &str| have.contains(&p);
        assert_eq!(
            parent_module("src/parser/env.rs", exists),
            Some(("src/parser/mod.rs".to_owned(), "env".to_owned()))
        );
        assert_eq!(
            parent_module("src/parser/mod.rs", exists),
            Some(("src/lib.rs".to_owned(), "parser".to_owned()))
        );
        assert_eq!(parent_module("src/lib.rs", exists), None);
    }
}
