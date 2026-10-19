use aver_core::Path;
use serde::{Deserialize, Serialize};
use similar::TextDiff;

use super::{FileEntry, FileKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffChange {
    Added,
    Modified,
    Deleted,
}

/// One changed file. `diff` is a unified diff with Git-style `a/` and `b/`
/// headers; it is empty for a mode-only change, and a one-line notice when
/// either side is not UTF-8.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: Path,
    pub change: DiffChange,
    pub old: Option<FileEntry>,
    pub new: Option<FileEntry>,
    pub binary: bool,
    pub diff: String,
}

pub(super) fn file_diff(
    path: &Path,
    old: Option<(FileEntry, &[u8])>,
    new: Option<(FileEntry, &[u8])>,
) -> FileDiff {
    let change = match (&old, &new) {
        (None, _) => DiffChange::Added,
        (_, None) => DiffChange::Deleted,
        _ => DiffChange::Modified,
    };
    let old_text = old.map_or(Some(""), |(_, b)| std::str::from_utf8(b).ok());
    let new_text = new.map_or(Some(""), |(_, b)| std::str::from_utf8(b).ok());
    let old_label = old.map_or("/dev/null".to_owned(), |_| format!("a/{path}"));
    let new_label = new.map_or("/dev/null".to_owned(), |_| format!("b/{path}"));
    let (binary, diff) = match (old_text, new_text) {
        (Some(a), Some(b)) => {
            let hunks = TextDiff::from_lines(a, b)
                .unified_diff()
                .context_radius(3)
                .header(&old_label, &new_label)
                .to_string();
            // The patch parser reads this mode line, so a created executable
            // round-trips through `patch` as executable.
            let created_exec = old.is_none() && new.is_some_and(|(f, _)| f.kind == FileKind::Exec);
            let diff = if created_exec && !hunks.is_empty() {
                format!("diff --git a/{path} b/{path}\nnew file mode 100755\n{hunks}")
            } else {
                hunks
            };
            (false, diff)
        }
        _ => (
            true,
            format!("Binary files {old_label} and {new_label} differ\n"),
        ),
    };
    FileDiff {
        path: path.clone(),
        change,
        old: old.map(|(f, _)| f),
        new: new.map(|(f, _)| f),
        binary,
        diff,
    }
}
