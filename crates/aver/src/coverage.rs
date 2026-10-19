//! Uncovered changes and affected checks (ARCHITECTURE.md §1.9.3): the
//! source files a head changed that no check of the verification could
//! exercise, and which checks a change could affect at all ([`affects`]),
//! so the others run on neither side.
//!
//! Every changed file is one of three kinds ([`kind`]): a source file of a
//! language Aver has presets for, by extension (JavaScript and TypeScript,
//! Python, Rust and Go, [`language`]); a file that is no code (documentation,
//! text, images, licences, anything under `docs/`, [`NOT_SOURCE`]) or data
//! and configuration a test may read ([`DATA`]); or source of a language
//! Aver does not know (Java, C, Ruby, shell, anything else). A check covers
//! the paths its `covers` patterns name. Without them, a check `aver.toml`
//! declares at the root covers every file: it runs the repository's whole
//! suite. A check Aver detected at the root, or any check with a `cwd`,
//! covers the source files of its image's language ([`Family`]) and
//! the data files under it, or every file under it when its image is not an
//! official image Aver knows (it may run anything). A change to Rust files
//! while the only check runs `npm test` in `web/`, or to a package no check
//! runs in, is uncovered: nothing measured it, so verification cannot call
//! the change safe, and the receipt says `UNPROVEN` instead of `PASS` or
//! `REVIEW`. Coverage is by path, not by what a test executes: a covered
//! file can still be one no test reaches.

use std::collections::BTreeSet;

use aver_core::{CheckDef, Environment, Path};

use crate::env::{within, Family, CONFIG_PATH};
use crate::judging::{self, Why};
use crate::workspace::ChangedFile;

/// The language of a source file, from its extension, or `None` for any
/// other file (documentation, configuration, data).
pub fn language(path: &str) -> Option<Family> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e)?;
    match ext {
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "vue" | "svelte" => {
            Some(Family::Node)
        }
        "py" | "pyi" => Some(Family::Python),
        "rs" => Some(Family::Rust),
        "go" => Some(Family::Go),
        _ => None,
    }
}

/// Extensions of files that are no code: documentation, text, images,
/// media and fonts. A change to one is never uncovered.
pub const NOT_SOURCE: &[&str] = &[
    "md", "markdown", "mdx", "rst", "txt", "adoc", "org", "png", "jpg", "jpeg", "gif", "svg",
    "webp", "ico", "bmp", "pdf", "mp3", "mp4", "mov", "webm", "wav", "woff", "woff2", "ttf", "otf",
    "eot",
];

/// Extensions of data and configuration a test may read; such a file is
/// covered by any check whose directory holds it.
pub const DATA: &[&str] = &[
    "json", "jsonc", "json5", "yaml", "yml", "toml", "ini", "cfg", "conf", "lock", "csv", "tsv",
    "xml", "html", "htm", "css", "scss", "sass", "less", "snap", "env", "sql", "graphql",
];

/// Manifests and lockfiles whose extension reads as source of another
/// language: Go's module files and Bun's binary lockfile.
pub const DATA_NAMES: &[&str] = &["go.mod", "go.sum", "go.work", "go.work.sum", "bun.lockb"];

/// What a changed file is to coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Source of a language Aver has presets for.
    Source(Family),
    /// Source of a language Aver does not know.
    OtherSource,
    /// Data or configuration.
    Data,
    /// Documentation, text, an image: never uncovered.
    NotSource,
}

/// What the file at `path` is to coverage, from its directory, name and
/// extension.
pub fn kind(path: &str) -> Kind {
    if let Some(family) = language(path) {
        return Kind::Source(family);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let upper = name.to_ascii_uppercase();
    let documentation = path.starts_with("docs/")
        || path.starts_with("doc/")
        || [
            "LICENSE",
            "LICENCE",
            "COPYING",
            "NOTICE",
            "AUTHORS",
            "CHANGELOG",
            "CODEOWNERS",
            "README",
            "TODO",
        ]
        .iter()
        .any(|n| upper.starts_with(n));
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        _ if documentation => Kind::NotSource,
        _ if DATA_NAMES.contains(&name) => Kind::Data,
        Some(e) if NOT_SOURCE.contains(&e) => Kind::NotSource,
        Some(e) if DATA.contains(&e) => Kind::Data,
        // Dotfiles (`.gitignore`, `.prettierrc`) configure tools, and a file
        // without an extension (`Makefile`, `VERSION`) is read as data.
        _ if name.starts_with('.') => Kind::Data,
        None => Kind::Data,
        _ => Kind::OtherSource,
    }
}

/// Whether `check`, run in `env`, could exercise the file at `path`.
///
/// `declared` says the checks come from `aver.toml`. A declared check at
/// the root without `covers` covers every file: a person wrote it to run
/// the whole repository's suite, whatever language its image is for (WP6
/// PY-3: a hand-written `pytest` check left every `src/` change uncovered).
/// A detected one covers only its image's language: CI's default `npm
/// test` says nothing about a Rust or Java change beside it (round 13a).
pub fn covers(env: &Environment, check: &CheckDef, path: &str, declared: bool) -> bool {
    if !check.covers.is_empty() {
        return check.covers.iter().any(|g| g.covers(path));
    }
    let cwd = match check.cwd.as_ref() {
        Some(cwd) => cwd.as_str(),
        None if declared => return true,
        None => "",
    };
    if !within(path, cwd) {
        return false;
    }
    match (Family::of(env.check_image(check)), kind(path)) {
        (None, _) => true,
        (Some(family), Kind::Source(of)) => of == family,
        (Some(_), Kind::Data | Kind::NotSource) => true,
        (Some(_), Kind::OtherSource) => false,
    }
}

/// Whether a head change to the file at `path` could change what `check`
/// reports, so a verification must run it (§1.9.3, affected checks). It
/// can when the file is
///
/// - the Aver configuration (`aver.toml`), which every check runs under;
/// - a lockfile of a dependency layer the check mounts (its `deps`, else
///   the environment's own lockfile);
/// - harness in the judging set (package manifests, runner and package
///   manager configuration, `Cargo.toml`, `build.rs`, `go.mod`) on the way
///   to the check's directory (root harness affects every check), or below
///   it when the file belongs to the check's ecosystem;
/// - in the check's scope: a path its `covers` patterns name, or, without
///   them, any file under its `cwd` (not only source files: tests read
///   fixtures and data). A check at the root without `covers`, as in a
///   single-package repository, is affected by every change.
///
/// `dirs` are the judging set's harness directories
/// ([`crate::judging::harness_dirs`]) and `at_base` says whether the file
/// exists at base.
pub fn affects(
    env: &Environment,
    dirs: &BTreeSet<String>,
    check: &CheckDef,
    path: &str,
    at_base: bool,
) -> bool {
    if path == CONFIG_PATH {
        return true;
    }
    let mounted = match &check.deps {
        Some(deps) => deps.iter().any(|p| p.as_str() == path),
        None => env
            .lockfile
            .as_ref()
            .is_some_and(|l| l.path.as_str() == path),
    };
    if mounted {
        return true;
    }
    let cwd = check.cwd.as_ref().map_or("", Path::as_str);
    let harness = matches!(
        judging::why(env, dirs, path, at_base),
        Some(Why::Config | Why::Manifest | Why::Harness | Why::TomlManifest | Why::GoMod)
    );
    if harness {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        // Harness on the way to the check's directory configures it; below
        // it, only harness of the check's own ecosystem does (a crate's
        // `Cargo.toml` for a Rust check at the root, not `site/tsconfig.json`).
        let own = match (
            Family::of(env.check_image(check)),
            harness_family(dir, name),
        ) {
            (Some(check), Some(file)) => check == file,
            _ => true,
        };
        if within(cwd, dir) || (own && within(dir, cwd)) {
            return true;
        }
    }
    if check.covers.is_empty() {
        within(path, cwd)
    } else {
        check.covers.iter().any(|g| g.covers(path))
    }
}

/// The ecosystem a harness file (`name` in directory `dir`) configures, or
/// `None` for one Aver cannot place.
fn harness_family(dir: &str, name: &str) -> Option<Family> {
    let dot = dir.rsplit('/').next().unwrap_or(dir);
    match name {
        "Cargo.toml" | "build.rs" => Some(Family::Rust),
        "config.toml" | "config" if dot == ".cargo" => Some(Family::Rust),
        "pyproject.toml" | "setup.cfg" | "pytest.ini" | "tox.ini" | "conftest.py" | "mypy.ini"
        | ".mypy.ini" => Some(Family::Python),
        "go.mod" => Some(Family::Go),
        _ if judging::is_harness_name(name) => Some(Family::Node),
        _ => None,
    }
}

/// The source files among `changed` (added, modified or deleted) that none
/// of `checks` covers, in path order.
pub fn uncovered(
    env: &Environment,
    checks: &[&CheckDef],
    changed: &[ChangedFile],
    declared: bool,
) -> Vec<Path> {
    changed
        .iter()
        .filter(|(path, _, _)| kind(path) != Kind::NotSource)
        .filter(|(path, _, _)| !checks.iter().any(|c| covers(env, c, path, declared)))
        .filter_map(|(path, _, _)| path.parse().ok())
        .collect()
}

/// The `covers` pattern naming every path in `paths`: their deepest
/// common directory, `<dir>/**`, or `**`.
pub fn covers_glob(paths: &[&str]) -> String {
    let mut common: Option<Vec<&str>> = None;
    for path in paths {
        let dirs: Vec<&str> = path.split('/').collect();
        let dirs = &dirs[..dirs.len() - 1];
        common = Some(match common {
            None => dirs.to_vec(),
            Some(c) => c
                .iter()
                .zip(dirs)
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| *a)
                .collect(),
        });
    }
    match common.unwrap_or_default().join("/") {
        dir if dir.is_empty() => "**".to_owned(),
        dir => format!("{dir}/**"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_covers_hint_is_the_common_directory() {
        assert_eq!(
            covers_glob(&["src/click/types.py", "src/click/core.py"]),
            "src/click/**"
        );
        assert_eq!(covers_glob(&["src/a/x.py", "src/b/y.py"]), "src/**");
        assert_eq!(covers_glob(&["lib/x.rb", "app.rb"]), "**");
    }

    #[test]
    fn languages_come_from_extensions() {
        assert_eq!(language("crates/aver/src/lib.rs"), Some(Family::Rust));
        assert_eq!(language("mcp/src/server.ts"), Some(Family::Node));
        assert_eq!(language("calc/test_calc.py"), Some(Family::Python));
        assert_eq!(language("cmd/main.go"), Some(Family::Go));
        assert_eq!(language("README.md"), None);
        assert_eq!(language("Makefile"), None);
        assert_eq!(language("action/run.sh"), None);
    }

    /// Red team hole 8: source of a language no preset knows counts.
    #[test]
    fn kinds_come_from_a_denylist_of_non_source_files() {
        assert_eq!(kind("src/main/java/app/Pricing.java"), Kind::OtherSource);
        assert_eq!(kind("native/hash.c"), Kind::OtherSource);
        assert_eq!(kind("lib/pricing.rb"), Kind::OtherSource);
        assert_eq!(kind("src/lib.rs"), Kind::Source(Family::Rust));
        for doc in [
            "README.md",
            "docs/guide.html",
            "LICENSE",
            "notes.txt",
            "logo.png",
        ] {
            assert_eq!(kind(doc), Kind::NotSource, "{doc}");
        }
        for data in [
            "config/app.yaml",
            "test/data.json",
            ".gitignore",
            "Cargo.lock",
            "Makefile",
            "go.mod",
            "svc/go.sum",
            "go.work",
            "bun.lockb",
        ] {
            assert_eq!(kind(data), Kind::Data, "{data}");
        }
    }
}
