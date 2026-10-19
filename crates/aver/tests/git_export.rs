mod git_support;

use aver::git::{self, ExportError, ImportError};
use aver::store::Store;
use aver_core::*;
use git_support::*;

fn tricky_entries() -> Vec<(Vec<u8>, F)> {
    vec![
        dir(
            "foo",
            vec![
                file("bar.ts", "export const bar = 1;\n"),
                dir("nested", vec![file("deep.txt", "deep\n")]),
            ],
        ),
        file("foo.txt", "foo text\n"),
        file("foo-bar", "dash\n"),
        file("foo0", "zero\n"),
        dir("bin", vec![exec("run.sh", "#!/bin/sh\nexit 0\n")]),
        link("latest", "foo.txt"),
        file("héllo wörld.md", "unicode\n"),
        file("quote\"name", "q\n"),
        file("new\nline", "n\n"),
        file("tab\there", "t\n"),
        file("back\\slash", "b\n"),
    ]
}

/// A tree equal to `root` with `path` set to `node`, or removed when `None`.
fn edit(store: &Store, root: &TreeId, path: &str, node: Option<Node>) -> TreeId {
    let segments: Vec<&str> = path.split('/').collect();
    edit_in(store, Some(*root), &segments, node).expect("root stays non-empty")
}

fn edit_in(
    store: &Store,
    dir: Option<TreeId>,
    segments: &[&str],
    node: Option<Node>,
) -> Option<TreeId> {
    let mut entries = dir
        .map(|d| store.get::<Tree>(&d).unwrap().entries.into_vec())
        .unwrap_or_default();
    let existing = entries.iter().position(|e| e.name.as_str() == segments[0]);
    let replacement = if segments.len() == 1 {
        node
    } else {
        let sub = existing.and_then(|i| match entries[i].node {
            Node::Tree(t) => Some(t),
            _ => None,
        });
        edit_in(store, sub, &segments[1..], node).map(Node::Tree)
    };
    if let Some(i) = existing {
        entries.remove(i);
    }
    if let Some(node) = replacement {
        entries.push(TreeEntry {
            name: p(segments[0]),
            node,
        });
    }
    if entries.is_empty() {
        return None;
    }
    Some(store.put(&Tree::new(Set::new(entries).unwrap())).unwrap())
}

/// Records a workspace transition from `from` to a snapshot of `root`.
fn workspace_commit(
    st: &Aver,
    from: SnapshotId,
    root: TreeId,
    message: &str,
    at_secs: i64,
) -> SnapshotId {
    merge_commit(st, &[from], root, message, at_secs)
}

fn merge_commit(
    st: &Aver,
    from: &[SnapshotId],
    root: TreeId,
    message: &str,
    at_secs: i64,
) -> SnapshotId {
    let env = st.store.get::<Snapshot>(&from[0]).unwrap().env.unwrap();
    let to = st.store.put(&Snapshot::new(root, env)).unwrap();
    let transition = Transition::sign(
        TransitionBody {
            schema: Schema::new(),
            from: from.to_vec(),
            to,
            via: Via::Workspace {
                workspace: WorkspaceId::generate(),
                attempt: None,
            },
            agent: st.agent(),
            message: message.to_owned(),
            at: Timestamp::from_unix_millis(at_secs * 1000 + 999).unwrap(),
        },
        &st.key,
    );
    st.store
        .tx(|tx| git::record_transition(tx, &transition))
        .unwrap();
    to
}

fn fsck(repo: &std::path::Path) {
    let out = git_cmd(repo)
        .args(["fsck", "--strict", "--no-dangling"])
        .output()
        .unwrap();
    assert!(
        out.status.success() && out.stdout.is_empty() && out.stderr.is_empty(),
        "fsck: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn round_trip_reproduces_the_original_tree_sha() {
    let fx = Fixture::new();
    let original = fx.commit_tree(&tricky_entries(), "tricky names");
    let st = Aver::new();
    let imported = st.import(fx.path(), "main");

    // The test only means something if the two orders really differ.
    let names: Vec<String> = st
        .store
        .get::<Tree>(&st.root(&imported.snapshot))
        .unwrap()
        .entries
        .iter()
        .map(|e| e.name.to_string())
        .collect();
    let pos = |n: &str| names.iter().position(|x| x == n).unwrap();
    assert!(pos("foo") < pos("foo-bar") && pos("foo-bar") < pos("foo.txt"));
    let git_order = git_str(&fx.repo, &["ls-tree", "--name-only", &original]);
    assert!(git_order.find("foo-bar").unwrap() < git_order.find("foo\n").unwrap());

    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("exported.git");
    let report = git::export(&st.store, &imported.snapshot, &target, "main").unwrap();
    assert_eq!(report.commits_written, 1);
    assert_eq!(
        git_str(
            &target,
            &["rev-parse", &format!("{}^{{tree}}", report.commit)]
        ),
        git_str(&fx.repo, &["rev-parse", &format!("{original}^{{tree}}")])
    );
    assert_eq!(
        git_str(&target, &["rev-parse", "main"]),
        report.commit.as_str()
    );
    assert_eq!(
        git_str(&target, &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
    fsck(&target);

    let message = git_str(&target, &["log", "-1", "--format=%B", "main"]);
    assert_eq!(
        message,
        format!("tricky names\n\nAver-Snapshot: {}", imported.snapshot)
    );
    let people = git_str(
        &target,
        &["log", "-1", "--format=%an|%ae|%cn|%at|%ct", "main"],
    );
    let agent = st.agent();
    assert_eq!(
        people,
        format!("{agent}|{agent}@aver.invalid|aver|1700000000|1700000000")
    );

    // Exporting to the repository that has the commit just points the branch at it.
    let back = git::export(&st.store, &imported.snapshot, &fx.repo, "imported").unwrap();
    assert_eq!(
        (back.commit.as_str(), back.commits_written),
        (original.as_str(), 0)
    );
    assert_eq!(git_str(&fx.repo, &["rev-parse", "imported"]), original);
}

#[test]
fn workspace_transitions_export_as_children_of_the_imported_commit() {
    let fx = Fixture::new();
    let original = fx.commit_tree(&tricky_entries(), "base");
    let st = Aver::new();
    let base = st.import(fx.path(), "main").snapshot;

    let deeper = st.store.put_blob(b"deeper\n").unwrap();
    let root1 = edit(
        &st.store,
        &st.root(&base),
        "foo/nested/deep.txt",
        Some(Node::File(deeper)),
    );
    let s1 = workspace_commit(&st, base, root1, "Deepen the nested file", 1_800_000_000);

    let first = git::export(&st.store, &s1, &fx.repo, "work").unwrap();
    let c1 = first.commit.as_str();
    assert_eq!(first.commits_written, 1);
    assert_eq!(
        git_str(&fx.repo, &["rev-parse", &format!("{c1}^")]),
        original
    );
    assert_eq!(
        git_str(&fx.repo, &["show", "--name-only", "--format=", c1]),
        "foo/nested/deep.txt"
    );
    assert_eq!(
        git_str(
            &fx.repo,
            &["cat-file", "blob", &format!("{c1}:foo/nested/deep.txt")]
        ),
        "deeper"
    );
    assert_eq!(
        git_str(&fx.repo, &["log", "-1", "--format=%B|%at", c1]),
        format!("Deepen the nested file\n\nAver-Snapshot: {s1}\n|1800000000")
    );
    fsck(&fx.repo);

    let again = git::export(&st.store, &s1, &fx.repo, "work").unwrap();
    assert_eq!((again.commit.as_str(), again.commits_written), (c1, 0));

    // Deletion, a file replaced by a directory, a new executable, a retargeted symlink.
    let new_file = st.store.put_blob(b"inside\n").unwrap();
    let script = st.store.put_blob(b"#!/bin/sh\necho new\n").unwrap();
    let target = st.store.put_blob(b"foo0/x").unwrap();
    let mut root2 = edit(&st.store, &root1, "foo-bar", None);
    root2 = edit(&st.store, &root2, "foo0", None);
    root2 = edit(&st.store, &root2, "foo0/x", Some(Node::File(new_file)));
    root2 = edit(&st.store, &root2, "bin/new.sh", Some(Node::Exec(script)));
    root2 = edit(&st.store, &root2, "latest", Some(Node::Symlink(target)));
    let s2 = workspace_commit(&st, s1, root2, "Reshape", 1_800_000_100);

    let second = git::export(&st.store, &s2, &fx.repo, "work").unwrap();
    let c2 = second.commit.as_str();
    assert_eq!(second.commits_written, 1);
    assert_eq!(git_str(&fx.repo, &["rev-parse", &format!("{c2}^")]), c1);
    assert_eq!(git_str(&fx.repo, &["rev-parse", "work"]), c2);
    fsck(&fx.repo);

    let mut expected = tricky_entries();
    expected.retain(|(name, _)| name != b"foo-bar" && name != b"foo0" && name != b"latest");
    expected.push(dir("foo0", vec![file("x", "inside\n")]));
    expected.push(link("latest", "foo0/x"));
    for (name, node) in &mut expected {
        match (name.as_slice(), node) {
            (b"foo", F::Dir(children)) => {
                children.retain(|(n, _)| n != b"nested");
                children.push(dir("nested", vec![file("deep.txt", "deeper\n")]));
            }
            (b"bin", F::Dir(children)) => children.push(exec("new.sh", "#!/bin/sh\necho new\n")),
            _ => {}
        }
    }
    let expected_tree = fx.tree(&expected);
    assert_eq!(
        git_str(&fx.repo, &["rev-parse", &format!("{c2}^{{tree}}")]),
        expected_tree
    );

    // A fresh repository gets the whole chain, root first.
    let out = tempfile::tempdir().unwrap();
    let fresh = out.path().join("fresh.git");
    let full = git::export(&st.store, &s2, &fresh, "main").unwrap();
    assert_eq!(full.commits_written, 3);
    let tree_of = |rev: String| git_str(&fresh, &["rev-parse", &format!("{rev}^{{tree}}")]);
    assert_eq!(tree_of(full.commit.to_string()), expected_tree);
    assert_eq!(
        tree_of(format!("{}~2", full.commit)),
        git_str(&fx.repo, &["rev-parse", &format!("{original}^{{tree}}")])
    );
    assert_eq!(git_str(&fresh, &["rev-list", "--count", "main"]), "3");
    fsck(&fresh);

    // The branch never moves backwards.
    let err = git::export(&st.store, &s1, &fx.repo, "work").unwrap_err();
    assert!(matches!(err, ExportError::NonFastForward { .. }), "{err}");
    assert_eq!(git_str(&fx.repo, &["rev-parse", "work"]), c2);
}

#[test]
fn snapshots_without_lineage_and_bad_branches_are_rejected() {
    let fx = Fixture::new();
    fx.commit_tree(&[file("a", "a\n")], "a");
    let st = Aver::new();
    let imported = st.import(fx.path(), "main");
    let out = tempfile::tempdir().unwrap();

    let err = git::export(
        &st.store,
        &imported.snapshot,
        &out.path().join("x.git"),
        "bad..name",
    )
    .unwrap_err();
    assert!(matches!(err, ExportError::InvalidBranch(_)), "{err}");
    assert!(!out.path().join("x.git").exists());

    let env = st
        .store
        .get::<Snapshot>(&imported.snapshot)
        .unwrap()
        .env
        .unwrap();
    let lonely_root = edit(
        &st.store,
        &st.root(&imported.snapshot),
        "b",
        Some(Node::File(st.store.put_blob(b"").unwrap())),
    );
    // A pure workspace snapshot or a verify head has no transition, yet a
    // workspace can be opened on it and committed. Both must export.
    let lonely = st.store.put(&Snapshot::new(lonely_root, env)).unwrap();
    let child_root = edit(
        &st.store,
        &lonely_root,
        "c",
        Some(Node::File(st.store.put_blob(b"c\n").unwrap())),
    );
    let child = workspace_commit(&st, lonely, child_root, "on a lonely base", 1_700_000_000);
    let repo = out.path().join("y.git");
    let report = git::export(&st.store, &child, &repo, "main").unwrap();
    assert_eq!(
        report.commits_written, 1,
        "the transition-less root is not mapped"
    );
    fsck(&repo);
    let parent = git_cmd(&repo)
        .args(["log", "--format=%an %at %s", "main~1"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&parent.stdout),
        format!("aver 0 Aver-Snapshot: {lonely}\n")
    );
    let again = git::export(&st.store, &child, &repo, "main").unwrap();
    assert_eq!(
        again.commit, report.commit,
        "the root commit is deterministic"
    );

    // The transition-less snapshot alone (a verify head) exports as that
    // root commit.
    let alone = out.path().join("z.git");
    let root_only = git::export(&st.store, &lonely, &alone, "main").unwrap();
    fsck(&alone);
    let head = git_cmd(&alone)
        .args(["rev-parse", "main"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        root_only.commit.to_string()
    );
}

#[test]
fn a_transition_with_two_parents_exports_as_a_merge_commit() {
    let fx = Fixture::new();
    let original = fx.commit_tree(&[file("a", "a\n"), file("b", "b\n")], "base");
    let st = Aver::new();
    let base = st.import(fx.path(), "main").snapshot;
    let root = st.root(&base);
    let a2 = st.store.put_blob(b"a2\n").unwrap();
    let b2 = st.store.put_blob(b"b2\n").unwrap();
    let left_root = edit(&st.store, &root, "a", Some(Node::File(a2)));
    let right_root = edit(&st.store, &root, "b", Some(Node::File(b2)));
    let both_root = edit(&st.store, &left_root, "b", Some(Node::File(b2)));
    let left = workspace_commit(&st, base, left_root, "left", 1_800_000_000);
    let right = workspace_commit(&st, base, right_root, "right", 1_800_000_001);
    let merged = merge_commit(&st, &[left, right], both_root, "merge", 1_800_000_002);

    let report = git::export(&st.store, &merged, &fx.repo, "merged").unwrap();
    assert_eq!(report.commits_written, 3);
    let parents = git_str(
        &fx.repo,
        &["rev-list", "--parents", "-1", report.commit.as_str()],
    );
    let parents: Vec<&str> = parents.split(' ').skip(1).collect();
    assert_eq!(parents.len(), 2);
    for parent in &parents {
        assert_eq!(
            git_str(&fx.repo, &["rev-parse", &format!("{parent}^")]),
            original
        );
    }
    assert_eq!(
        git_str(&fx.repo, &["log", "-1", "--format=%s", parents[0]]),
        "left"
    );
    assert_eq!(
        git_str(
            &fx.repo,
            &["cat-file", "blob", &format!("{}:b", report.commit)]
        ),
        "b2"
    );
    fsck(&fx.repo);
}

#[test]
fn the_snapshot_trailer_joins_the_message_trailer_block() {
    let fx = Fixture::new();
    fx.commit_tree(&[file("a", "a\n")], "base");
    let st = Aver::new();
    let base = st.import(fx.path(), "main").snapshot;
    let a2 = st.store.put_blob(b"a2\n").unwrap();
    let root = edit(&st.store, &st.root(&base), "a", Some(Node::File(a2)));
    let message = "Fix a\n\nWhy it changed.\n\nAver-Task: t1\nAver-Attempt: a1\nAver-Receipt: r1\n";
    let head = workspace_commit(&st, base, root, message, 1_800_000_000);

    let report = git::export(&st.store, &head, &fx.repo, "work").unwrap();
    let body = git_str(
        &fx.repo,
        &["log", "-1", "--format=%B", report.commit.as_str()],
    );
    let out = git_cmd(&fx.repo)
        .args(["interpret-trailers", "--parse"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(body.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    let parsed = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        parsed,
        format!("Aver-Task: t1\nAver-Attempt: a1\nAver-Receipt: r1\nAver-Snapshot: {head}\n")
    );
}

#[test]
fn an_import_with_exclusions_exports_its_own_pruned_tree() {
    let fx = Fixture::new();
    let original = fx.commit_tree(
        &[
            file("app.ts", "export {};\n"),
            dir("private", vec![file(".env", "TOKEN=abc\n")]),
        ],
        "with a secret",
    );
    let st = Aver::new();
    let exclude = [p("private")];
    let mut options = st.options(fx.path(), &original);
    options.exclude = &exclude;
    let imported = git::import(&st.store, &options).unwrap();

    let report = git::export(&st.store, &imported.snapshot, &fx.repo, "pruned").unwrap();
    assert_ne!(report.commit.as_str(), original);
    assert_eq!(
        git_str(
            &fx.repo,
            &["ls-tree", "-r", "--name-only", report.commit.as_str()]
        ),
        "app.ts"
    );
    // Re-importing the same commit with the same exclusions is still idempotent.
    let again = git::import(&st.store, &options).unwrap();
    assert!(again.already_imported);
    assert_eq!(again.transition, imported.transition);
}

#[test]
fn long_histories_export_without_deep_recursion() {
    let fx = Fixture::new();
    fx.commit_tree(&[file("n", "0\n")], "base");
    let st = Aver::new();
    let mut head = st.import(fx.path(), "main").snapshot;
    const DEPTH: usize = 3_000;
    for i in 1..=DEPTH {
        let blob = st.store.put_blob(format!("{i}\n").as_bytes()).unwrap();
        let root = edit(&st.store, &st.root(&head), "n", Some(Node::File(blob)));
        head = workspace_commit(
            &st,
            head,
            root,
            &format!("step {i}"),
            1_800_000_000 + i as i64,
        );
    }
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("long.git");
    // The 2 MiB stack of a Tokio worker thread, where `Api::export` runs exports.
    let report = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn({
            let target = target.clone();
            move || git::export(&st.store, &head, &target, "main").map(|r| r.commits_written)
        })
        .unwrap()
        .join()
        .expect("export must not overflow the stack")
        .unwrap();
    assert_eq!(report, DEPTH + 1);
    assert_eq!(
        git_str(&target, &["rev-list", "--count", "main"]),
        (DEPTH + 1).to_string()
    );
}

#[test]
fn a_lineage_that_loops_is_an_error() {
    let fx = Fixture::new();
    fx.commit_tree(&[file("a", "a\n")], "a");
    let st = Aver::new();
    let base = st.import(fx.path(), "main").snapshot;
    let root = st.root(&base);
    let b = edit(
        &st.store,
        &root,
        "b",
        Some(Node::File(st.store.put_blob(b"").unwrap())),
    );
    let c = edit(
        &st.store,
        &root,
        "c",
        Some(Node::File(st.store.put_blob(b"").unwrap())),
    );
    let env = st.store.get::<Snapshot>(&base).unwrap().env.unwrap();
    let p = st.store.put(&Snapshot::new(c, env)).unwrap();
    // `s` is first reached from `p`, and `p` first from `s`.
    let s = workspace_commit(&st, p, b, "s from p", 1_800_000_000);
    workspace_commit(&st, s, c, "p from s", 1_800_000_001);
    let out = tempfile::tempdir().unwrap();
    let err = git::export(&st.store, &s, &out.path().join("loop.git"), "main").unwrap_err();
    assert!(matches!(err, ExportError::Cycle(_)), "{err}");
}

#[test]
fn an_existing_directory_that_is_not_a_repository_never_reaches_an_enclosing_one() {
    let fx = Fixture::new();
    fx.commit_tree(&[file("a", "a\n")], "a");
    let st = Aver::new();
    let imported = st.import(fx.path(), "main");
    // `out` is a plain directory inside the source repository's git dir;
    // Git discovery would otherwise walk up and write there.
    let out = fx.repo.join("out");
    std::fs::create_dir(&out).unwrap();
    let err = git::export(&st.store, &imported.snapshot, &out, "leaked").unwrap_err();
    assert!(matches!(err, ExportError::Git(_)), "{err}");
    let leaked = git_cmd(&fx.repo)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/leaked"])
        .status()
        .unwrap()
        .success();
    assert!(
        !leaked,
        "export wrote a branch into the enclosing repository"
    );

    // Import does not discover an enclosing repository either.
    let err = git::import(&st.store, &st.options(out.to_str().unwrap(), "main")).unwrap_err();
    assert!(
        matches!(err, ImportError::Git(_) | ImportError::Rev(_)),
        "{err}"
    );
}
