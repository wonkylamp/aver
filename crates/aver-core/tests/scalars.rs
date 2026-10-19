use std::str::FromStr;

use aver_core::*;

fn accepts<T: FromStr + ToString>(inputs: &[&str]) {
    for s in inputs {
        let parsed = T::from_str(s).unwrap_or_else(|_| panic!("{s:?} should parse"));
        assert_eq!(parsed.to_string(), *s, "display round trips");
    }
}

fn rejects<T: FromStr>(inputs: &[&str]) {
    for s in inputs {
        assert!(T::from_str(s).is_err(), "{s:?} should be rejected");
    }
}

#[test]
fn content_ids_parse_only_their_own_canonical_form() {
    let id = BlobId::of_bytes(b"x");
    let text = id.to_string();
    assert_eq!(text.len(), "blob_".len() + 52);
    assert_eq!(BlobId::from_str(&text), Ok(id));

    let hash = &text["blob_".len()..];
    rejects::<BlobId>(&[
        &format!("tree_{hash}"),
        &format!("blob{hash}"),
        &format!("blob_{}", &hash[..51]),
        &format!("blob_{hash}a"),
        &format!("blob_{}", hash.to_ascii_uppercase()),
        &format!("blob_{}1", &hash[..51]),
        "",
    ]);
    // The 52nd character carries 4 padding bits that must be zero.
    let last = hash.as_bytes()[51];
    let flipped = if last == b'a' { 'b' } else { 'a' };
    let non_zero_trailing = format!("blob_{}{}", &hash[..51], flipped);
    assert!(BlobId::from_str(&non_zero_trailing).is_err());
    rejects::<TreeId>(&[&text]);
}

#[test]
fn ids_order_like_their_text() {
    let mut ids: Vec<BlobId> = (0u32..200)
        .map(|n| BlobId::of_bytes(&n.to_le_bytes()))
        .collect();
    ids.sort();
    let texts: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let mut sorted = texts.clone();
    sorted.sort();
    assert_eq!(texts, sorted);
}

#[test]
fn ids_serialize_as_strings() {
    let id = SnapshotId::from_str(&format!("snap_{}", "a".repeat(52))).unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{id}\""));
    let back: SnapshotId = serde_json::from_str(&format!("\"{id}\"")).unwrap();
    assert_eq!(back, id);
    assert!(serde_json::from_str::<SnapshotId>("1").is_err());
}

#[test]
fn live_ids_are_prefixed_canonical_ulids() {
    let ws = WorkspaceId::generate();
    let text = ws.to_string();
    assert!(text.starts_with("ws_"));
    assert_eq!(text.len(), 3 + 26);
    assert_eq!(WorkspaceId::from_str(&text), Ok(ws));
    let ulid = &text[3..];
    rejects::<WorkspaceId>(&[
        &format!("att_{ulid}"),
        &format!("ws_{}", ulid.to_ascii_lowercase()),
        &format!("ws_{}", &ulid[..25]),
        "ws_8ZZZZZZZZZZZZZZZZZZZZZZZZZ",
    ]);
    accepts::<AttemptId>(&["att_01K7A0000000000000000000CD"]);
    accepts::<LeaseId>(&["lease_7ZZZZZZZZZZZZZZZZZZZZZZZZZ"]);
}

#[test]
fn paths_follow_the_repo_relative_rules() {
    accepts::<Path>(&[
        "a",
        "src/index.ts",
        ".github/workflows/ci.yml",
        "a b/ü.txt",
        "..a/b..",
        ".gitignore",
    ]);
    rejects::<Path>(&[
        "",
        "/a",
        "a/",
        "a//b",
        ".",
        "a/./b",
        "a/../b",
        "..",
        ".git",
        "x/.GIT/config",
        "a\0b",
        &"a".repeat(4097),
    ]);
    accepts::<Path>(&[&"a".repeat(4096)]);
}

#[test]
fn names_are_single_segments() {
    accepts::<Name>(&["index.ts", ".env", "...", ".gitkeep"]);
    rejects::<Name>(&["", ".", "..", ".Git", "a/b", "a\0"]);
}

#[test]
fn check_and_criterion_ids_follow_their_patterns() {
    accepts::<CheckId>(&["unit", "0", "a.b_c-d", &"a".repeat(64)]);
    rejects::<CheckId>(&["", "Unit", "-a", ".a", "a b", "a:b", &"a".repeat(65)]);
    accepts::<CriterionId>(&["div-zero", "A1", "x.y_z", &"Z".repeat(64)]);
    rejects::<CriterionId>(&["", "-a", "a b", "a/b", &"Z".repeat(65)]);
}

#[test]
fn test_check_ids_join_a_check_id_and_a_test_path() {
    let id = TestCheckId::from_str("unit::calc > nested > [crit:div-zero] throws").unwrap();
    assert_eq!(id.check().as_str(), "unit");
    assert_eq!(id.test_path(), "calc > nested > [crit:div-zero] throws");
    let built = TestCheckId::new(&"unit".parse().unwrap(), "a::b").unwrap();
    assert_eq!(built.test_path(), "a::b");
    accepts::<TestCheckId>(&[&format!("unit::{}", "t".repeat(1024))]);
    rejects::<TestCheckId>(&[
        "unit",
        "unit::",
        "Unit::x",
        "::x",
        &format!("unit::{}", "t".repeat(1025)),
    ]);
    accepts::<EntryId>(&["unit", "unit::a > b"]);
    rejects::<EntryId>(&["Unit", "unit::"]);
}

#[test]
fn timestamps_are_exact_utc_milliseconds() {
    accepts::<Timestamp>(&[
        "2026-10-10T12:00:00.000Z",
        "1970-01-01T00:00:00.000Z",
        "0000-01-01T00:00:00.000Z",
        "9999-12-31T23:59:59.999Z",
        "2024-02-29T00:00:00.000Z",
    ]);
    rejects::<Timestamp>(&[
        "2026-10-10T12:00:00Z",
        "2026-10-10T12:00:00.0Z",
        "2026-10-10T12:00:00.0000Z",
        "2026-10-10T12:00:00.000+00:00",
        "2026-10-10t12:00:00.000z",
        "2026-10-10 12:00:00.000Z",
        "2026-13-10T12:00:00.000Z",
        "2025-02-29T00:00:00.000Z",
        "2026-10-10T24:00:00.000Z",
        "2026-10-10T12:00:60.000Z",
        "+2026-10-10T12:00:00.000Z",
    ]);
    let t = Timestamp::from_unix_millis(1_791_201_600_123).unwrap();
    assert_eq!(t.to_string(), "2026-10-05T12:00:00.123Z");
    assert_eq!(
        Timestamp::from_str(&t.to_string()).unwrap().unix_millis(),
        1_791_201_600_123
    );
    assert!(Timestamp::from_unix_millis(-62_167_219_200_001).is_err());
    assert!(Timestamp::now() > t);
}

#[test]
fn digests_git_shas_and_ref_names() {
    accepts::<Digest>(&[&format!("sha256:{}", "0f".repeat(32))]);
    rejects::<Digest>(&[
        &format!("sha256:{}", "0F".repeat(32)),
        &format!("sha512:{}", "0f".repeat(32)),
        &format!("sha256:{}", "0f".repeat(31)),
    ]);
    accepts::<GitSha>(&[&"a1".repeat(20)]);
    rejects::<GitSha>(&[&"a1".repeat(21), &"A1".repeat(20)]);
    accepts::<RefName>(&["trunk", "ledger/ref:trunk", "feature/x-1.2"]);
    rejects::<RefName>(&[
        "",
        "/trunk",
        "trunk/",
        "a//b",
        "a/../b",
        "a b",
        &"r".repeat(256),
    ]);
}

#[test]
fn report_paths_live_under_aver_out() {
    accepts::<ReportPath>(&["/aver/out/unit.xml", "/aver/out/a/b.xml"]);
    rejects::<ReportPath>(&[
        "/aver/out/",
        "/aver/out/../x",
        "/tmp/unit.xml",
        "aver/out/x",
    ]);
}

#[test]
fn text_is_bounded_by_bytes_and_truncates_on_char_boundaries() {
    assert!(Text::<4>::from_str("abcd").is_ok());
    assert!(Text::<4>::from_str("abcde").is_err());
    assert!(Text::<4>::from_str("ééé").is_err());
    assert_eq!(Text::<5>::truncated("ééé").as_str(), "éé");
}

#[test]
fn path_globs_cover_matching_paths_and_everything_under_them() {
    accepts::<PathGlob>(&[
        "test",
        "test/**",
        "**/*.test.js",
        "src/?.js",
        "*",
        "**",
        "a/**/b",
    ]);
    rejects::<PathGlob>(&[
        "", "/a", "a/", "a//b", ".", "a/../b", "a**", "**b", "a/***", "a\\b",
    ]);
    let covers = |glob: &str, path: &str| glob.parse::<PathGlob>().unwrap().covers(path);

    assert!(covers("test", "test"));
    assert!(
        covers("test", "test/unit/a.js"),
        "a directory covers its files"
    );
    assert!(!covers("test", "tests/a.js"));
    assert!(!covers("test", "src/test/a.js"));
    assert!(covers("test/**", "test/a.js"));
    assert!(covers("test/**", "test/unit/deep/a.js"));
    assert!(
        covers("**/*.test.js", "a.test.js"),
        "'**' matches no segments"
    );
    assert!(covers("**/*.test.js", "src/math/div.test.js"));
    assert!(!covers("**/*.test.js", "src/math/div.js"));
    assert!(covers("src/*.js", "src/a.js"));
    assert!(
        !covers("src/*.js", "src/lib/a.js"),
        "'*' stays in its segment"
    );
    assert!(covers("src/?.js", "src/é.js"), "'?' is one character");
    assert!(!covers("src/?.js", "src/ab.js"));
    assert!(covers("a/**/b", "a/b"));
    assert!(covers("a/**/b", "a/x/y/b/c.txt"));
    assert!(!covers("a/**/b", "a/x/y/c"));
    assert!(covers("*", "anything/at/all"));
    assert!(covers("package.json", "package.json"));
    assert!(!covers("package.json", "sub/package.json"));
    assert!(covers("[x].js", "[x].js"), "brackets are literal");
}

#[test]
fn path_glob_overlap_means_some_path_is_covered_by_both() {
    let overlaps = |a: &str, b: &str| {
        let (a, b) = (
            a.parse::<PathGlob>().unwrap(),
            b.parse::<PathGlob>().unwrap(),
        );
        let one = a.overlaps(&b);
        assert_eq!(one, b.overlaps(&a), "overlap is symmetric: {a} {b}");
        one
    };
    assert!(
        overlaps("src", "src/a.js"),
        "a directory overlaps its files"
    );
    assert!(overlaps("src/a.js", "src/**"));
    assert!(overlaps("test/**", "test"));
    assert!(overlaps("src/*.js", "src/a*"));
    assert!(!overlaps("src/*.js", "src/*.ts"));
    assert!(!overlaps("lib", "src"));
    assert!(!overlaps("src/lib", "src/bin/x"));
    assert!(overlaps("**/*.test.js", "test/a.test.js"));
    assert!(overlaps("**/*.test.js", "**/a.*"));
    assert!(!overlaps("a?c", "abd"));
    assert!(overlaps("a?c", "a*"));
    assert!(overlaps("**", "anything"));
    assert!(overlaps("a/**/b", "a/x/*/b/c"));
    assert!(!overlaps("a/*/b", "a/x/y/b"));
    assert!(
        overlaps("*", "deep/path"),
        "'*' names a top-level directory"
    );
}
