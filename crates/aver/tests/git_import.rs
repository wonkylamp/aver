mod git_support;

use aver::env::{self, StaticResolver};
use aver::git::{
    self, ImportError, OversizePolicy, RefPrecondition, RefUpdate, SecretPolicy, ViolationReason,
};
use aver::secrets::Severity;
use aver::store::StoreError;
use aver_core::id::kind;
use aver_core::*;
use git_support::*;

fn simple(fx: &Fixture, content: &str) -> String {
    fx.commit_tree(
        &[
            file("README.md", content),
            dir("src", vec![file("index.ts", "export const x = 1;\n")]),
        ],
        "simple",
    )
}

fn trunk_update(expect: RefPrecondition) -> Option<RefUpdate> {
    Some(RefUpdate {
        name: p("trunk"),
        expect,
    })
}

#[test]
fn import_builds_merkle_trees_with_kinds_and_signed_lineage() {
    let fx = Fixture::new();
    let commit = fx.commit_tree(
        &[
            dir("foo", vec![dir("nested", vec![file("deep.txt", "deep\n")])]),
            file("foo.txt", "text\n"),
            exec("run.sh", "#!/bin/sh\necho hi\n"),
            link("latest", "foo.txt"),
            file("package-lock.json", "{}\n"),
        ],
        "first commit\n\nwith a body",
    );
    let st = Aver::new();
    let report = st.import(fx.path(), "main");

    assert_eq!(report.commit.as_str(), commit);
    assert_eq!(report.files, 5);
    assert_eq!(report.bytes, (5 + 5 + 18 + 7 + 3) as u64);
    // 5 distinct blobs, 3 trees, env, snapshot, transition, ledger entry.
    assert_eq!(report.new_objects, 12);
    assert!(!report.already_imported);

    let root = st.root(&report.snapshot);
    let node = |path: &str| env::lookup(&st.store, &root, &p(path)).unwrap().unwrap();
    assert_eq!(node("foo.txt"), Node::File(BlobId::of_bytes(b"text\n")));
    assert_eq!(
        node("run.sh"),
        Node::Exec(BlobId::of_bytes(b"#!/bin/sh\necho hi\n"))
    );
    assert_eq!(node("latest"), Node::Symlink(BlobId::of_bytes(b"foo.txt")));
    assert_eq!(
        env::read_file(&st.store, &root, &p("foo/nested/deep.txt")).unwrap(),
        Some(b"deep\n".to_vec())
    );

    let environment: Environment = st.store.get(&report.env).unwrap();
    assert_eq!(
        environment.lockfile.unwrap().path.as_str(),
        "package-lock.json"
    );

    let transition: Transition = st.store.get(&report.transition).unwrap();
    transition.verify(&st.key.verifying_key()).unwrap();
    assert_eq!(transition.body.to, report.snapshot);
    assert!(transition.body.from.is_empty());
    assert_eq!(transition.body.agent, st.agent());
    assert_eq!(transition.body.message, "first commit\n\nwith a body\n");
    assert_eq!(transition.body.at.unix_millis(), 1_700_000_000_000);
    let Via::GitImport {
        repo,
        commit: via_commit,
    } = &transition.body.via
    else {
        panic!("expected a git-import transition");
    };
    assert_eq!(via_commit.as_str(), commit);
    assert_eq!(
        std::path::Path::new(repo),
        fx.repo.canonicalize().unwrap().as_path()
    );
}

#[test]
fn rejection_lists_every_violation_without_secret_bytes_and_writes_nothing() {
    let fx = Fixture::new();
    let aws = format!("{}{}", "AKIA", "QWERTYUIOPASDFGH");
    let commit = fx.commit_tree(
        &[
            file("clean.txt", "nothing to see\n"),
            (
                b"vendor".to_vec(),
                F::Gitlink("0123456789abcdef0123456789abcdef01234567"),
            ),
            (b"bad\xffname".to_vec(), F::File(b"x".to_vec())),
            dir(".GIT", vec![file("config", "[core]\n")]),
            dir(
                "src",
                vec![file("keys.ts", &format!("export const k = '{aws}';\n"))],
            ),
            file(".env", "DATABASE_URL=postgres://x\n"),
        ],
        "bad",
    );
    let st = Aver::new();
    let mut options = st.options(fx.path(), &commit);
    options.update_ref = trunk_update(RefPrecondition::Any);
    let err = git::import(&st.store, &options).unwrap_err();
    let ImportError::Rejected(violations) = &err else {
        panic!("expected a rejection, got {err}");
    };
    let got: Vec<(&str, &ViolationReason)> = violations
        .iter()
        .map(|v| (v.path.as_str(), &v.reason))
        .collect();
    assert_eq!(got.len(), 5, "{got:?}");
    assert_eq!(got[0].0, ".GIT/config");
    assert!(matches!(got[0].1, ViolationReason::InvalidPath(_)));
    assert_eq!(got[1], (".env", &ViolationReason::Secret("dotenv-file")));
    assert_eq!(got[2].0, "bad\u{fffd}name");
    assert!(matches!(got[2].1, ViolationReason::InvalidPath(_)));
    assert_eq!(
        got[3],
        ("src/keys.ts", &ViolationReason::Secret("aws-access-key-id"))
    );
    assert_eq!(got[4], ("vendor", &ViolationReason::Submodule));

    let message = err.to_string();
    assert!(message.contains("src/keys.ts") && message.contains("vendor"));
    assert!(!message.contains(&aws));
    assert!(!message.contains("postgres"));

    assert!(!st
        .store
        .has(&BlobId::of_bytes(b"nothing to see\n"))
        .unwrap());
    assert!(st
        .store
        .get_ref::<kind::Snapshot>(&p("trunk"))
        .unwrap()
        .is_none());
}

#[test]
fn the_warn_policy_imports_and_reports_possible_secrets_without_their_bytes() {
    let fx = Fixture::new();
    let aws = format!("{}{}", "AKIA", "QWERTYUIOPASDFGH");
    let commit = fx.commit_tree(
        &[
            dir(
                "src",
                vec![file("keys.ts", &format!("export const k = '{aws}';\n"))],
            ),
            dir("tests", vec![file(".env", "DATABASE_URL=postgres://x\n")]),
            file(".env.test.example", "API_KEY=changeme\n"),
            (
                b"nodes.db".to_vec(),
                F::File([b"SQLite\0".as_slice(), aws.as_bytes()].concat()),
            ),
        ],
        "fixtures",
    );
    let st = Aver::new();
    let mut options = st.options(fx.path(), &commit);
    options.secrets = SecretPolicy::Warn;
    let report = git::import(&st.store, &options).unwrap();
    let got: Vec<(&str, &str, Severity)> = report
        .secrets
        .iter()
        .map(|f| (f.path.as_str(), f.pattern, f.severity))
        .collect();
    assert_eq!(
        got,
        [
            ("src/keys.ts", "aws-access-key-id", Severity::High),
            ("tests/.env", "dotenv-file", Severity::Low),
        ]
    );
    assert!(!format!("{:?}", report.secrets).contains(&aws));
    let root = st.root(&report.snapshot);
    assert!(env::lookup(&st.store, &root, &p("tests/.env"))
        .unwrap()
        .is_some());

    let strict = st.options(fx.path(), &commit);
    let err = git::import(&st.store, &strict).unwrap_err();
    assert!(matches!(err, ImportError::Rejected(v) if v.len() == 2));
}

#[test]
fn oversize_blobs_become_empty_placeholders_only_when_asked() {
    let fx = Fixture::new();
    let big = vec![b'g'; MAX_BLOB_BYTES + 1];
    let commit = fx.commit_tree(
        &[
            dir("img", vec![(b"demo.gif".to_vec(), F::File(big))]),
            file("empty.txt", ""),
            file("index.js", "module.exports = 1;\n"),
        ],
        "big gif",
    );
    let st = Aver::new();
    let err = git::import(&st.store, &st.options(fx.path(), &commit)).unwrap_err();
    assert!(matches!(
        &err,
        ImportError::Rejected(v) if matches!(v[0].reason, ViolationReason::TooLarge(_))
    ));

    let mut options = st.options(fx.path(), &commit);
    options.oversize = OversizePolicy::Placeholder;
    let report = git::import(&st.store, &options).unwrap();
    assert_eq!(report.placeholders.len(), 1);
    assert_eq!(report.placeholders[0].path, "img/demo.gif");
    assert_eq!(report.placeholders[0].bytes, MAX_BLOB_BYTES as u64 + 1);
    let root = st.root(&report.snapshot);
    assert_eq!(
        env::read_file(&st.store, &root, &p("img/demo.gif")).unwrap(),
        Some(Vec::new())
    );
    assert_eq!(
        env::read_file(&st.store, &root, &p("index.js")).unwrap(),
        Some(b"module.exports = 1;\n".to_vec())
    );

    // The placeholder is not the Git blob, so exporting must re-create the
    // commit rather than claim Git's SHAs; the real empty file stays mapped.
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("export.git");
    let exported = git::export(&st.store, &report.snapshot, &target, "main").unwrap();
    assert_ne!(exported.commit.as_str(), commit);
    let listing = git_str(&target, &["ls-tree", "-r", "-l", exported.commit.as_str()]);
    assert!(listing.contains("img/demo.gif"), "{listing}");
    assert!(
        listing
            .lines()
            .any(|l| l.contains("img/demo.gif") && l.contains(" 0\t")),
        "{listing}"
    );
}

#[test]
fn excluded_paths_are_left_out_unchecked() {
    let fx = Fixture::new();
    let commit = fx.commit_tree(
        &[
            file("app.ts", "export {};\n"),
            dir("secrets", vec![file(".env", "TOKEN=abc\n")]),
            file("secrets.md", "docs\n"),
        ],
        "with secrets",
    );
    let st = Aver::new();
    let exclude = [p("secrets")];
    let mut options = st.options(fx.path(), &commit);
    options.exclude = &exclude;
    let report = git::import(&st.store, &options).unwrap();
    assert_eq!(report.files, 2);
    let root = st.root(&report.snapshot);
    assert!(env::lookup(&st.store, &root, &p("secrets"))
        .unwrap()
        .is_none());
    assert!(env::lookup(&st.store, &root, &p("secrets.md"))
        .unwrap()
        .is_some());
}

#[test]
fn reimporting_a_commit_is_idempotent_including_through_a_url() {
    let fx = Fixture::new();
    let commit = simple(&fx, "hello\n");
    let st = Aver::new();
    let mut options = st.options(fx.path(), &commit);
    options.update_ref = trunk_update(RefPrecondition::Absent);
    let first = git::import(&st.store, &options).unwrap();
    let ledger_ref: RefName = p("ledger/ref:trunk");
    let ledger_before = st.store.get_ref::<kind::Ledger>(&ledger_ref).unwrap();

    // The ref already points at the snapshot, so even `Absent` is satisfied.
    let second = git::import(&st.store, &options).unwrap();
    assert!(second.already_imported);
    assert_eq!(second.snapshot, first.snapshot);
    assert_eq!(second.transition, first.transition);
    assert_eq!(second.new_objects, 0);
    assert_eq!(second.ledger, None);
    assert_eq!(
        st.store.get_ref::<kind::Ledger>(&ledger_ref).unwrap(),
        ledger_before
    );

    let url = format!("file://{}", fx.repo.display());
    let third = st.import(&url, "main");
    assert!(third.already_imported);
    assert_eq!(third.transition, first.transition);
}

#[test]
fn url_sources_are_cloned_into_a_scratch_dir_that_is_removed() {
    let fx = Fixture::new();
    let commit = simple(&fx, "via url\n");
    let st = Aver::new();
    let url = format!("file://{}", fx.repo.display());
    let report = st.import(&url, &commit);
    let transition: Transition = st.store.get(&report.transition).unwrap();
    assert_eq!(
        transition.body.via,
        Via::GitImport {
            repo: url,
            commit: p(&commit)
        }
    );
    let leftovers: Vec<_> = std::fs::read_dir(st.home.path().join("tmp"))
        .unwrap()
        .collect();
    assert!(leftovers.is_empty(), "scratch clone was not removed");
}

#[test]
fn identical_trees_yield_identical_snapshots_across_commits_and_stores() {
    let fx = Fixture::new();
    let tree = fx.tree(&[file("a.txt", "same\n"), dir("d", vec![file("b", "b\n")])]);
    let one = fx.commit(&tree, &[], "one");
    let two = fx.commit(&tree, &[&one], "two, a different commit with the same tree");

    let st = Aver::new();
    let a = st.import(fx.path(), &one);
    let b = st.import(fx.path(), &two);
    assert_eq!(a.snapshot, b.snapshot);
    assert_ne!(a.transition, b.transition);
    assert!(!b.already_imported);
    assert_eq!(b.new_objects, 2, "only a transition and a ledger entry");

    let fresh = Aver::new();
    assert_eq!(fresh.import(fx.path(), &two).snapshot, a.snapshot);
}

#[test]
fn ledger_entries_chain_per_scope_and_ref_updates_are_compare_and_set() {
    let fx = Fixture::new();
    let c1 = simple(&fx, "v1\n");
    let c2 = simple(&fx, "v2\n");
    let c3 = simple(&fx, "v3\n");
    let st = Aver::new();
    let ledger_ref: RefName = p("ledger/ref:trunk");

    let mut options = st.options(fx.path(), &c1);
    options.update_ref = trunk_update(RefPrecondition::Absent);
    let r1 = git::import(&st.store, &options).unwrap();

    let mut options = st.options(fx.path(), &c2);
    options.update_ref = trunk_update(RefPrecondition::Is(r1.snapshot));
    let r2 = git::import(&st.store, &options).unwrap();

    let trunk = st
        .store
        .get_ref::<kind::Snapshot>(&p("trunk"))
        .unwrap()
        .unwrap();
    assert_eq!((trunk.target, trunk.version), (r2.snapshot, 2));
    let head = st
        .store
        .get_ref::<kind::Ledger>(&ledger_ref)
        .unwrap()
        .unwrap();
    assert_eq!(Some(head.target), r2.ledger);
    let e2: LedgerEntry = st.store.get(&head.target).unwrap();
    let e1: LedgerEntry = st.store.get(&r1.ledger.unwrap()).unwrap();
    e1.verify(&st.key.verifying_key()).unwrap();
    e2.verify(&st.key.verifying_key()).unwrap();
    assert_eq!(e1.body.prev, None);
    assert_eq!(e2.body.prev, r1.ledger);
    assert_eq!(e2.body.scope, "ref:trunk");
    assert_eq!(e2.body.kind, LedgerKind::Import);
    assert_eq!(e2.body.base, Some(r1.snapshot));
    assert_eq!(e2.body.head, Some(r2.snapshot));

    // A stale precondition fails and rolls the whole import back.
    let mut options = st.options(fx.path(), &c3);
    options.update_ref = trunk_update(RefPrecondition::Is(r1.snapshot));
    let err = git::import(&st.store, &options).unwrap_err();
    assert!(
        matches!(err, ImportError::Store(StoreError::RefConflict { .. })),
        "{err}"
    );
    assert!(!st.store.has(&BlobId::of_bytes(b"v3\n")).unwrap());
    assert_eq!(
        st.store
            .get_ref::<kind::Ledger>(&ledger_ref)
            .unwrap()
            .unwrap()
            .target,
        head.target
    );

    // Imports without a ref chain in their own scope.
    let r3 = st.import(fx.path(), &c3);
    let e3: LedgerEntry = st.store.get(&r3.ledger.unwrap()).unwrap();
    assert_eq!((e3.body.scope.as_str(), e3.body.prev), ("imports", None));

    // A different commit with trunk's tree writes a new transition but does
    // not move trunk, so its entry belongs to `imports`, not `ref:trunk`.
    let same_tree = fx.commit_tree(
        &[
            file("README.md", "v2\n"),
            dir("src", vec![file("index.ts", "export const x = 1;\n")]),
        ],
        "same tree, another commit",
    );
    assert_ne!(same_tree, c2);
    let mut options = st.options(fx.path(), &same_tree);
    options.update_ref = trunk_update(RefPrecondition::Any);
    let r4 = git::import(&st.store, &options).unwrap();
    assert_eq!(r4.snapshot, r2.snapshot);
    let e4: LedgerEntry = st.store.get(&r4.ledger.unwrap()).unwrap();
    assert_eq!(e4.body.scope, "imports");
    assert_eq!(
        st.store
            .get_ref::<kind::Ledger>(&ledger_ref)
            .unwrap()
            .unwrap()
            .target,
        head.target,
        "ref:trunk's chain is untouched"
    );
}

#[test]
fn aver_toml_with_a_tag_and_nested_lockfile_pins_the_environment() {
    let fx = Fixture::new();
    let commit = fx.commit_tree(
        &[
            file(
                "aver.toml",
                "[env]\nimage = \"node:22\"\nlockfile = \"app/package-lock.json\"\n",
            ),
            dir("app", vec![file("package-lock.json", "{\"v\":1}\n")]),
        ],
        "configured",
    );
    let digest: Digest = p(&format!("sha256:{}", "ab".repeat(32)));
    let mut st = Aver::new();
    st.resolver = StaticResolver::default().with("node:22", digest.clone());
    let report = st.import(fx.path(), &commit);
    let environment: Environment = st.store.get(&report.env).unwrap();
    assert_eq!(environment.image.reference, "node:22");
    assert_eq!(environment.image.digest, digest);
    assert_eq!(
        environment.lockfile,
        Some(Lockfile {
            path: p("app/package-lock.json"),
            blob: BlobId::of_bytes(b"{\"v\":1}\n"),
        })
    );

    let unresolvable = Aver::new();
    let err = git::import(
        &unresolvable.store,
        &unresolvable.options(fx.path(), &commit),
    )
    .unwrap_err();
    assert!(
        matches!(err, ImportError::Env(env::EnvError::Resolve { .. })),
        "{err}"
    );
}

#[test]
fn a_missing_revision_or_source_is_an_error() {
    let fx = Fixture::new();
    simple(&fx, "x\n");
    let st = Aver::new();
    let err = git::import(&st.store, &st.options(fx.path(), "no-such-branch")).unwrap_err();
    assert!(matches!(err, ImportError::Rev(_)), "{err}");
    let err = git::import(&st.store, &st.options("/definitely/not/here", "main")).unwrap_err();
    assert!(matches!(err, ImportError::Source(_)), "{err}");
}

#[test]
fn malformed_trees_with_duplicate_names_are_rejected_not_trusted() {
    // `git mktree` accepts duplicate names; fsck would not. Repository content
    // is untrusted, so such a tree must fail import instead of panicking or
    // silently dropping one of the entries.
    for entries in [
        vec![file("a", "file\n"), dir("a", vec![file("b", "under\n")])],
        vec![dir("a", vec![file("b", "under\n")]), file("a", "file\n")],
        vec![file("a", "one\n"), file("a", "two\n")],
    ] {
        let fx = Fixture::new();
        let commit = fx.commit_tree(&entries, "duplicate names");
        let st = Aver::new();
        let err = git::import(&st.store, &st.options(fx.path(), &commit)).unwrap_err();
        let ImportError::Rejected(violations) = &err else {
            panic!("expected a rejection, got {err}");
        };
        assert!(
            violations
                .iter()
                .any(|v| v.path == "a" && matches!(v.reason, ViolationReason::InvalidPath(_))),
            "{violations:?}"
        );
    }
}

#[test]
fn a_failed_clone_never_echoes_url_credentials() {
    let st = Aver::new();
    let url = "https://someone:hunter2-not-a-real-token@127.0.0.1:1/repo.git";
    let err = git::import(&st.store, &st.options(url, "main")).unwrap_err();
    assert!(matches!(err, ImportError::Git(_)), "{err}");
    let message = format!("{err} {err:?}");
    assert!(!message.contains("hunter2"), "{message}");
    assert!(message.contains("127.0.0.1:1/repo.git"), "{message}");
}

/// A working repository (not bare) with one commit on `main`.
fn working_repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let p = dir.path().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
    git(
        dir.path(),
        &["init", "--quiet", "--initial-branch=main"],
        b"",
    );
    git(dir.path(), &["add", "-A"], b"");
    git(dir.path(), &["commit", "--quiet", "-m", "base"], b"");
    dir
}

#[test]
fn reimporting_an_unchanged_head_reads_neither_the_tree_nor_the_blobs() {
    let repo = working_repo(&[("README.md", "hi\n"), ("src/a.js", "exports.a = 1;\n")]);
    let source = repo.path().to_str().unwrap();
    let st = Aver::new();
    let first = st.import(source, "HEAD");
    assert!(!first.already_imported);

    // Refs packed and every object gone: only a memoized import succeeds.
    git(repo.path(), &["pack-refs", "--all"], b"");
    assert!(!repo.path().join(".git/refs/heads/main").exists());
    for entry in std::fs::read_dir(repo.path().join(".git/objects")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().len() == 2 {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
    let again = st.import(source, "HEAD");
    assert!(again.already_imported);
    assert_eq!(again.snapshot, first.snapshot);
    assert_eq!(again.transition, first.transition);
    assert_eq!(again.commit, first.commit);
    assert_eq!(again.files, 2);
    assert_eq!(again.bytes, first.bytes);
    assert_eq!(again.message, first.message);
    assert_eq!(again.ledger, None);
}

#[test]
fn a_reimport_under_other_detection_derives_its_own_environment() {
    let repo = working_repo(&[
        ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
        ("test/a.test.js", "require('node:test')('a', () => {});\n"),
    ]);
    let source = repo.path().to_str().unwrap();
    let st = Aver::new();
    let off = st.import(source, "HEAD");
    let mut options = st.options(source, "HEAD");
    options.detection = env::Detection::Presets;
    let presets = git::import(&st.store, &options).unwrap();

    let fresh = Aver::new();
    let mut options = fresh.options(source, "HEAD");
    options.detection = env::Detection::Presets;
    let expected = git::import(&fresh.store, &options).unwrap();
    assert_ne!(off.env, expected.env, "detection changes the environment");
    assert_eq!(presets.env, expected.env);
    assert_eq!(presets.snapshot, expected.snapshot);
}
