//! `$AVER_HOME/integrations.json` (ARCHITECTURE.md §2.12): the
//! repositories `aver integrate` and `aver setup` set up, by target, with
//! the integration rev and the Aver version that wrote them. A newer binary
//! reads it to tell, once per repository and build, that an integration is
//! stale; doctor compares the hooks themselves.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::INTEGRATION_REV;
use crate::keys::Home;

/// The file's name in the home.
pub const FILE: &str = "integrations.json";

/// One target set up in one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The repository's root, canonical.
    pub repo: PathBuf,
    /// `claude`, `cursor`, `codex`, `gemini`, `vscode` or `agents-md`.
    pub target: String,
    pub rev: u32,
    /// The Aver version that wrote it.
    pub version: String,
    /// The last build that warned about this entry being stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warned_build: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    #[serde(default)]
    pub entries: Vec<Entry>,
}

impl Installed {
    pub fn load(home: &Home) -> Self {
        std::fs::read(home.root().join(FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the file whole (temporary file, then rename), so a reader
    /// never sees half of it.
    pub fn save(&self, home: &Home) -> std::io::Result<()> {
        let path = home.root().join(FILE);
        let tmp = home
            .root()
            .join(format!("{FILE}.{}.tmp", std::process::id()));
        let mut text = serde_json::to_string_pretty(self).expect("entries serialize");
        text.push('\n');
        std::fs::create_dir_all(home.root())?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    }

    /// The entries for the repository containing `dir`.
    pub fn for_dir<'a>(&'a self, dir: &'a Path) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries
            .iter()
            .filter(move |e| dir.starts_with(&e.repo))
    }
}

/// Notes that `target` is set up in `repo` at this build's rev.
pub fn record(home: &Home, repo: &Path, target: &str) -> std::io::Result<()> {
    let mut all = Installed::load(home);
    all.entries
        .retain(|e| !(e.repo == repo && e.target == target));
    all.entries.push(Entry {
        repo: repo.to_owned(),
        target: target.to_owned(),
        rev: INTEGRATION_REV,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        warned_build: None,
    });
    all.entries
        .sort_by(|a, b| (&a.repo, &a.target).cmp(&(&b.repo, &b.target)));
    all.save(home)
}

/// Forgets every target of `repo` (after `aver setup --remove`).
pub fn forget(home: &Home, repo: &Path) -> std::io::Result<()> {
    let mut all = Installed::load(home);
    let before = all.entries.len();
    all.entries.retain(|e| e.repo != repo);
    if all.entries.len() == before {
        return Ok(());
    }
    all.save(home)
}

/// This binary: its version and when it was built (the executable's
/// modification time), since every build of one version shares the
/// version string.
pub fn build_id() -> String {
    let mtime = std::env::current_exe()
        .and_then(std::fs::metadata)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("{}+{mtime}", env!("CARGO_PKG_VERSION"))
}

/// The one-time warning for a command run in `dir`: the stale
/// integrations of its repository that this build has not warned about
/// yet, recorded as warned. `None` when there is nothing to say.
pub fn warn_once(home: &Home, dir: &Path) -> Option<String> {
    let mut all = Installed::load(home);
    let build = build_id();
    let mut stale = Vec::new();
    for e in all.entries.iter_mut() {
        if dir.starts_with(&e.repo)
            && e.rev != INTEGRATION_REV
            && e.warned_build.as_deref() != Some(build.as_str())
        {
            e.warned_build = Some(build.clone());
            stale.push((e.target.clone(), e.rev, e.repo.clone()));
        }
    }
    let (_, rev, repo) = stale.first()?.clone();
    let _ = all.save(home);
    let targets: Vec<String> = stale.into_iter().map(|(t, ..)| t).collect();
    Some(format!(
        "aver: the {} integration in {} is from an older Aver (rev {rev}; this aver writes rev {INTEGRATION_REV}); run `aver integrate {}` there to update it",
        targets.join(", "),
        repo.display(),
        targets[0]
    ))
}

/// What a hook says when settings call it with `rev` (`None`: from before
/// integrations carried a rev) and this build writes another.
pub fn stale_hook_text(rev: Option<u32>) -> Option<String> {
    if rev == Some(INTEGRATION_REV) {
        return None;
    }
    let was = match rev {
        Some(r) if r > INTEGRATION_REV => {
            format!("rev {r}, newer than this aver's {INTEGRATION_REV}")
        }
        Some(r) => format!("rev {r}; this aver writes rev {INTEGRATION_REV}"),
        None => format!("from before integration revs; this aver writes rev {INTEGRATION_REV}"),
    };
    Some(format!(
        "Aver's Claude Code hooks in this repository are out of date ({was}), so some of Aver's help may be missing or wrong. Ask the user to run `aver integrate claude` in the repository (it shows the change before writing)."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_entry_warns_once_per_build_and_a_fresh_one_never() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::new(tmp.path());
        let repo = Path::new("/r/calc");
        record(&home, repo, "claude").unwrap();
        assert_eq!(warn_once(&home, &repo.join("src")), None);

        let mut all = Installed::load(&home);
        all.entries[0].rev = 0;
        all.save(&home).unwrap();
        let text = warn_once(&home, &repo.join("src")).unwrap();
        assert!(text.contains("aver integrate claude"), "{text}");
        assert_eq!(warn_once(&home, repo), None, "once per build");
        assert_eq!(warn_once(&home, Path::new("/r/other")), None);

        record(&home, repo, "claude").unwrap();
        assert_eq!(Installed::load(&home).entries.len(), 1);
        forget(&home, repo).unwrap();
        assert!(Installed::load(&home).entries.is_empty());
    }

    #[test]
    fn hooks_at_this_rev_are_not_stale() {
        assert_eq!(stale_hook_text(Some(INTEGRATION_REV)), None);
        assert!(stale_hook_text(None)
            .unwrap()
            .contains("aver integrate claude"));
        assert!(stale_hook_text(Some(INTEGRATION_REV + 1))
            .unwrap()
            .contains("newer"));
    }
}
