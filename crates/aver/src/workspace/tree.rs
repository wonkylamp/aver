//! Tree walking and the overlay merge shared by reads, listing and snapshots.

use std::collections::BTreeMap;
use std::sync::Arc;

use aver_core::{Name, Node, Object, Set, Tree, TreeEntry, TreeId};

use super::{Change, FileEntry};
use crate::store::StoreError;

pub(super) type Loader<'a> = dyn Fn(&TreeId) -> Result<Arc<Tree>, StoreError> + 'a;

/// The node at `path` (non-empty, `/`-separated) under `root`.
pub(super) fn node_at(
    load: &Loader<'_>,
    root: &TreeId,
    path: &str,
) -> Result<Option<Node>, StoreError> {
    let mut tree = load(root)?;
    let mut segments = path.split('/').peekable();
    while let Some(segment) = segments.next() {
        let Some(entry) = tree.entries.get(segment) else {
            return Ok(None);
        };
        if segments.peek().is_none() {
            return Ok(Some(entry.node));
        }
        match entry.node {
            Node::Tree(id) => tree = load(&id)?,
            _ => return Ok(None),
        }
    }
    Ok(None)
}

/// Calls `visit(path, file)` for every file under `tree`, depth first in name
/// order, with paths prefixed by `prefix/`. Stops early when `visit` returns
/// `true`, and reports whether it did.
pub(super) fn walk_files(
    load: &Loader<'_>,
    tree: &TreeId,
    prefix: &str,
    visit: &mut dyn FnMut(String, FileEntry) -> Result<bool, StoreError>,
) -> Result<bool, StoreError> {
    let tree = load(tree)?;
    for entry in &tree.entries {
        let path = format!("{prefix}/{}", entry.name);
        let stop = match FileEntry::of_node(&entry.node) {
            Some(file) => visit(path, file)?,
            None => match entry.node {
                Node::Tree(sub) => walk_files(load, &sub, &path, visit)?,
                _ => unreachable!("non-file nodes are trees"),
            },
        };
        if stop {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A file whose entry differs between two trees: its path and its entry on
/// each side, `None` where it does not exist.
pub type ChangedFile = (String, Option<FileEntry>, Option<FileEntry>);

/// Appends every file that differs between directories `base` and `head`
/// (`None` where the directory does not exist) to `out`, in name order, with
/// paths prefixed by `prefix/` unless `prefix` is empty. Subtrees with equal
/// ids are skipped, so the cost follows the size of the change. A file
/// replaced by a directory (or the reverse) is a deleted file plus every file
/// added under the directory.
pub(super) fn diff_files(
    load: &Loader<'_>,
    base: Option<&TreeId>,
    head: Option<&TreeId>,
    prefix: &str,
    out: &mut Vec<ChangedFile>,
) -> Result<(), StoreError> {
    if base == head {
        return Ok(());
    }
    let base = base.map(load).transpose()?;
    let head = head.map(load).transpose()?;
    let node = |tree: &Option<Arc<Tree>>, name: &str| {
        tree.as_ref()
            .and_then(|t| t.entries.get(name))
            .map(|e| e.node)
    };
    let names: std::collections::BTreeSet<&str> = base
        .iter()
        .chain(head.iter())
        .flat_map(|t| t.entries.iter().map(|e| e.name.as_str()))
        .collect();
    let subtree = |n: Option<Node>| match n {
        Some(Node::Tree(id)) => Some(id),
        _ => None,
    };
    for name in names {
        let (b, h) = (node(&base, name), node(&head, name));
        if b == h {
            continue;
        }
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let (bf, hf) = (
            b.and_then(|n| FileEntry::of_node(&n)),
            h.and_then(|n| FileEntry::of_node(&n)),
        );
        if bf != hf {
            out.push((path.clone(), bf, hf));
        }
        let (bt, ht) = (subtree(b), subtree(h));
        if bt.is_some() || ht.is_some() {
            diff_files(load, bt.as_ref(), ht.as_ref(), &path, out)?;
        }
    }
    Ok(())
}

/// Overlay changes arranged by path segment.
#[derive(Default)]
pub(super) struct Trie {
    leaf: Option<Change>,
    children: BTreeMap<String, Trie>,
}

impl Trie {
    pub(super) fn insert(&mut self, rel_path: &str, change: Change) {
        let mut node = self;
        for segment in rel_path.split('/') {
            node = node.children.entry(segment.to_owned()).or_default();
        }
        node.leaf = Some(change);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

fn collision(path: &str) -> StoreError {
    StoreError::Corrupt {
        id: "overlay".into(),
        reason: format!("{path} is both a file and a directory"),
    }
}

/// Applies `changes` to `base` (a directory, `None` when it does not exist),
/// returning the merged directory, possibly empty. Subdirectories that become
/// empty are pruned; unchanged subtrees keep their ids. Every new non-empty
/// subtree is appended to `new_trees`; the returned tree is not.
pub(super) fn merge(
    load: &Loader<'_>,
    base: Option<&Tree>,
    changes: &Trie,
    new_trees: &mut Vec<Tree>,
) -> Result<Tree, StoreError> {
    let mut entries: BTreeMap<String, Node> = base
        .map(|tree| {
            tree.entries
                .iter()
                .map(|e| (e.name.as_str().to_owned(), e.node))
                .collect()
        })
        .unwrap_or_default();
    for (name, child) in &changes.children {
        let base_entry = entries.remove(name);
        let mut merged = match (&child.leaf, base_entry) {
            (Some(Change::Put(file)), _) => Some(file.node()),
            (Some(Change::Delete), _) | (None, Some(Node::Tree(_)) | None) => None,
            (None, Some(file)) => Some(file),
        };
        if !child.children.is_empty() {
            let base_sub = match base_entry {
                Some(Node::Tree(id)) => Some(load(&id)?),
                _ => None,
            };
            let sub = merge(load, base_sub.as_deref(), child, new_trees)?;
            if !sub.entries.is_empty() {
                if merged.is_some() {
                    return Err(collision(name));
                }
                merged = Some(Node::Tree(sub.id()));
                new_trees.push(sub);
            }
        } else if matches!(base_entry, Some(Node::Tree(_))) {
            return Err(collision(name));
        }
        if let Some(node) = merged {
            entries.insert(name.clone(), node);
        }
    }
    let entries = entries
        .into_iter()
        .map(|(name, node)| {
            let name: Name =
                name.parse()
                    .map_err(|e: aver_core::ParseError| StoreError::Corrupt {
                        id: "overlay".into(),
                        reason: e.to_string(),
                    })?;
            Ok(TreeEntry { name, node })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok(Tree::new(
        Set::new(entries).expect("names are unique map keys"),
    ))
}
