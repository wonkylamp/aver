//! Tree (ARCHITECTURE.md §1.3): a directory listing, entries sorted by name bytes.

use serde::{Deserialize, Serialize, Serializer};

use crate::id::{kind, BlobId, TreeId};
use crate::object::{impl_object, Schema};
use crate::scalar::Name;
use crate::set::{Set, SetItem};
use crate::ParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tree {
    pub schema: Schema<Tree>,
    pub entries: Set<TreeEntry>,
}

impl_object!(Tree, kind::Tree, "aver.tree/v0");

impl Tree {
    pub fn new(entries: Set<TreeEntry>) -> Self {
        Self {
            schema: Schema::new(),
            entries,
        }
    }
}

/// One entry: a name and what it points to. Serialized as
/// `{"name", "kind", "id"}`, with the id kind fixed by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawEntry")]
pub struct TreeEntry {
    pub name: Name,
    pub node: Node,
}

/// What a tree entry points to. Maps one-to-one onto Git modes 100644,
/// 100755, 120000 and 040000. A symlink's target is stored as a blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    File(BlobId),
    Exec(BlobId),
    Symlink(BlobId),
    Tree(TreeId),
}

impl SetItem for TreeEntry {
    type Key = str;
    fn key(&self) -> &str {
        self.name.as_str()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawKind {
    File,
    Exec,
    Symlink,
    Tree,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    name: Name,
    kind: RawKind,
    id: String,
}

impl TryFrom<RawEntry> for TreeEntry {
    type Error = ParseError;
    fn try_from(raw: RawEntry) -> Result<Self, ParseError> {
        let node = match raw.kind {
            RawKind::File => Node::File(raw.id.parse()?),
            RawKind::Exec => Node::Exec(raw.id.parse()?),
            RawKind::Symlink => Node::Symlink(raw.id.parse()?),
            RawKind::Tree => Node::Tree(raw.id.parse()?),
        };
        Ok(Self {
            name: raw.name,
            node,
        })
    }
}

impl Serialize for TreeEntry {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let (kind, id) = match &self.node {
            Node::File(id) => (RawKind::File, id.to_string()),
            Node::Exec(id) => (RawKind::Exec, id.to_string()),
            Node::Symlink(id) => (RawKind::Symlink, id.to_string()),
            Node::Tree(id) => (RawKind::Tree, id.to_string()),
        };
        let mut st = s.serialize_struct("TreeEntry", 3)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("kind", &kind)?;
        st.serialize_field("id", &id)?;
        st.end()
    }
}
