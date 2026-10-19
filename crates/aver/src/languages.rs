//! Which ecosystems a checkout holds, read from its project files, so the
//! text Aver gives an agent (the brief, `aver setup`, `aver doctor`) speaks
//! of npm only in a Node repository and names `pytest`, `cargo test` or
//! `go test` where those run. It looks where generic mode looks
//! ([`crate::presets`]): the root and directories at most
//! [`crate::presets::MAX_DEPTH`] levels down, skipping hidden ones and
//! [`crate::presets::SKIP`]. A guess for wording only: what runs is the
//! environment's business.

use std::path::Path;

use crate::env::Family;

/// The ecosystems with a project file under `dir`, in the order Node,
/// Python, Rust, Go; empty when there is none or `dir` cannot be read.
pub fn families_in(dir: &Path) -> Vec<Family> {
    let mut found = [false; 4];
    walk(dir, 0, &mut found);
    [Family::Node, Family::Python, Family::Rust, Family::Go]
        .into_iter()
        .zip(found)
        .filter_map(|(f, yes)| yes.then_some(f))
        .collect()
}

fn walk(dir: &Path, depth: usize, found: &mut [bool; 4]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < super::presets::MAX_DEPTH
                && !name.starts_with('.')
                && !super::presets::SKIP.contains(&name)
            {
                walk(&entry.path(), depth + 1, found);
            }
            continue;
        }
        // A Node package below the root counts only with its own lockfile,
        // as generic mode requires; the root's package.json alone gets the
        // default `npm test` check.
        let node = name == "package-lock.json" || (depth == 0 && name == "package.json");
        let python = matches!(
            name,
            "pyproject.toml" | "setup.py" | "setup.cfg" | "uv.lock"
        ) || (name.starts_with("requirements") && name.ends_with(".txt"));
        let rust = matches!(name, "Cargo.toml" | "Cargo.lock");
        let go = name == "go.mod";
        for (i, yes) in [node, python, rust, go].into_iter().enumerate() {
            found[i] |= yes;
        }
    }
}

/// The ecosystems' names, joined for a sentence: `Python and Rust`.
pub fn names(families: &[Family]) -> String {
    let words: Vec<&str> = families.iter().map(|f| name(*f)).collect();
    match words.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub fn name(family: Family) -> &'static str {
    match family {
        Family::Node => "Node",
        Family::Python => "Python",
        Family::Rust => "Rust",
        Family::Go => "Go",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_files_name_the_ecosystems_and_skipped_directories_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(families_in(root).is_empty());
        std::fs::write(root.join("pyproject.toml"), "").unwrap();
        assert_eq!(families_in(root), [Family::Python]);
        // A dependency or fixture directory's project files are not the
        // repository's.
        for skipped in ["node_modules/x", "fixtures/go", ".hidden"] {
            std::fs::create_dir_all(root.join(skipped)).unwrap();
            std::fs::write(root.join(skipped).join("go.mod"), "").unwrap();
            std::fs::write(root.join(skipped).join("package-lock.json"), "").unwrap();
        }
        assert_eq!(families_in(root), [Family::Python]);
        std::fs::create_dir_all(root.join("crates/core")).unwrap();
        std::fs::write(root.join("crates/core/Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(root.join("site")).unwrap();
        std::fs::write(root.join("site/package.json"), "{}").unwrap();
        assert_eq!(
            families_in(root),
            [Family::Python, Family::Rust],
            "a nested package.json without a lockfile gets no check"
        );
        std::fs::write(root.join("site/package-lock.json"), "{}").unwrap();
        assert_eq!(
            families_in(root),
            [Family::Node, Family::Python, Family::Rust]
        );
        assert_eq!(names(&families_in(root)), "Node, Python and Rust");
    }
}
