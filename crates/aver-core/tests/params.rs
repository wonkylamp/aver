use aver_core::params::{pair, split};

#[test]
fn split_finds_pytest_parameters_and_go_subtests_in_the_last_segment() {
    let cases: &[(&str, Option<(&str, &str)>)] = &[
        (
            "py::tests/test_t.py > test_at[2026-10-17T10:01]",
            Some(("py::tests/test_t.py > test_at", "[2026-10-17T10:01]")),
        ),
        (
            "py::tests/test_t.py > TestA > test_x[a/b]",
            Some(("py::tests/test_t.py > TestA > test_x", "[a/b]")),
        ),
        (
            "go::example.com/calc > TestX/0xc000123",
            Some(("go::example.com/calc > TestX", "/0xc000123")),
        ),
        (
            "go::example.com/calc > TestX/case/0xc0001",
            Some(("go::example.com/calc > TestX", "/case/0xc0001")),
        ),
        ("go::example.com/calc > TestX", None),
        (
            "unit::test/a.test.js > adds [two]",
            Some(("unit::test/a.test.js > adds ", "[two]")),
        ),
        ("unit::test/a.test.js > [x]", None),
        ("unit::keeps", None),
        ("tsc::src/a.ts(3,1) TS2322", None),
    ];
    for (id, expected) in cases {
        assert_eq!(split(id), *expected, "split({id:?})");
    }
}

#[test]
fn pair_matches_one_function_whose_suffix_changed() {
    let base = ["py::t.py > test_at[2026-10-16T09:00]"];
    let head = ["py::t.py > test_at[2026-10-17T10:01]"];
    assert_eq!(pair(&base, &head), vec![(base[0], head[0])]);
}

#[test]
fn pair_matches_several_suffixes_in_sorted_order_only_when_the_counts_agree() {
    let base = [
        "go::m > TestX/0xc0002",
        "go::m > TestX/0xc0001",
        "go::m > TestY/a",
    ];
    let head = ["go::m > TestX/0xc0009", "go::m > TestX/0xc0008"];
    assert_eq!(
        pair(&base, &head),
        vec![
            ("go::m > TestX/0xc0001", "go::m > TestX/0xc0008"),
            ("go::m > TestX/0xc0002", "go::m > TestX/0xc0009"),
        ],
        "TestY has nothing on the other side"
    );
    let one_gone = ["go::m > TestX/0xc0009"];
    assert!(
        pair(&base, &one_gone).is_empty(),
        "two lost and one gained cannot be told apart"
    );
}

#[test]
fn pair_never_matches_another_function_or_an_id_without_a_suffix() {
    assert!(pair(&["py::t.py > test_a[1]"], &["py::t.py > test_b[1]"]).is_empty());
    assert!(pair(&["unit::gone"], &["unit::gone2"]).is_empty());
    assert!(pair(&[], &["py::t.py > test_a[1]"]).is_empty());
}
