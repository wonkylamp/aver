//! The criteria block of CI mode (ARCHITECTURE.md §2.9) and the Markdown
//! helpers that keep repository text inert in the summary.

use aver::ci::spec::{from_file, from_markdown, Spec, SpecError};
use aver::ci::summary::{code, code_block, escape};
use aver::ci::FailOn;
use aver_core::Verdict;

fn ids(spec: &Spec) -> Vec<(String, String)> {
    spec.criteria
        .iter()
        .map(|c| (c.id.to_string(), c.text.clone()))
        .collect()
}

fn globs(spec: &Spec) -> Vec<String> {
    spec.protected_paths.iter().map(|g| g.to_string()).collect()
}

#[test]
fn a_pull_request_block_declares_criteria_and_protected_paths() {
    let body = "Implements divide.\r\n\r\n```aver\r\n- div-basic: divide returns the quotient\r\n\r\n- div-zero:   divide throws on division by zero  \r\nprotected: test/**\r\nprotected: package.json\r\n```\r\nThanks!";
    let spec = from_markdown(body).unwrap().unwrap();
    assert_eq!(
        ids(&spec),
        [
            ("div-basic".into(), "divide returns the quotient".into()),
            (
                "div-zero".into(),
                "divide throws on division by zero".into()
            ),
        ]
    );
    assert_eq!(globs(&spec), ["package.json", "test/**"]);
}

#[test]
fn no_block_is_regression_only_and_other_fences_are_ignored() {
    assert_eq!(from_markdown("").unwrap(), None);
    assert_eq!(from_markdown("Just a fix.").unwrap(), None);
    let body = "```js\n- a: not criteria\n```\n~~~\nprotected: x\n~~~";
    assert_eq!(from_markdown(body).unwrap(), None);

    let example = "How to declare criteria:\n````markdown\n```aver\n- a: example only\n```\n````\n";
    assert_eq!(
        from_markdown(example).unwrap(),
        None,
        "an aver fence inside a longer fence is an example, not a block"
    );
}

#[test]
fn tilde_fences_and_info_after_the_word_aver_work() {
    let spec = from_markdown("~~~ aver criteria\n- a: x\n~~~")
        .unwrap()
        .unwrap();
    assert_eq!(ids(&spec), [("a".into(), "x".into())]);
    let only_protected = from_markdown("```aver\nprotected: src/**\n```")
        .unwrap()
        .unwrap();
    assert!(only_protected.criteria.is_empty());
    assert_eq!(globs(&only_protected), ["src/**"]);
}

#[test]
fn malformed_blocks_name_the_line_and_the_problem() {
    let cases: &[(&str, SpecError)] = &[
        (
            "intro\n```aver\n- a: x\nmaybe: y\n```",
            SpecError::Line {
                line: 4,
                reason: "expected `- <id>: <text>` or `protected: <glob>`, found \"maybe: y\"".into(),
            },
        ),
        (
            "```aver\n- bad id: x\n```",
            SpecError::Line {
                line: 2,
                reason: "\"bad id\" is not a criterion id ([A-Za-z0-9][A-Za-z0-9._-], at most 64 characters)".into(),
            },
        ),
        (
            "```aver\n- a: x\n- a: y\n```",
            SpecError::Line {
                line: 3,
                reason: "criterion a is declared twice".into(),
            },
        ),
        (
            "```aver\n- a:\n```",
            SpecError::Line {
                line: 2,
                reason: "criterion a has no text".into(),
            },
        ),
        (
            "```aver\n- no colon here\n```",
            SpecError::Line {
                line: 2,
                reason: "a criterion needs the form `- <id>: <text>`".into(),
            },
        ),
        ("```aver\n\n```", SpecError::Empty(1)),
        ("text\n```aver\n- a: x\n", SpecError::Unclosed(2)),
        (
            "```aver\n- a: x\n```\n```aver\n- b: y\n```",
            SpecError::Several(vec![1, 4]),
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(from_markdown(body).unwrap_err(), *expected, "{body:?}");
    }
    let err = from_markdown("```aver\nprotected: ../up\n```").unwrap_err();
    assert!(
        matches!(&err, SpecError::Line { line: 2, reason } if reason.contains("../up")),
        "{err}"
    );
    assert_eq!(
        err.to_string().split(':').next(),
        Some("line 2 of the aver block")
    );
}

#[test]
fn a_criteria_file_holds_the_lines_with_or_without_the_fence() {
    let bare = from_file("- a: first\nprotected: test/**\n").unwrap();
    let fenced = from_file("# Criteria\n```aver\n- a: first\nprotected: test/**\n```\n").unwrap();
    assert_eq!(bare, fenced);
    assert_eq!(ids(&bare), [("a".into(), "first".into())]);
    assert_eq!(from_file("\n\n").unwrap_err(), SpecError::EmptyFile);
    assert!(matches!(
        from_file("- a: x\nnonsense\n").unwrap_err(),
        SpecError::Line { line: 2, .. }
    ));
}

#[test]
fn fail_on_decides_which_verdicts_fail_the_run() {
    use Verdict::*;
    let table = [
        (FailOn::FailOwn, [false, true, false, false]),
        (FailOn::NotPass, [false, true, true, true]),
        (FailOn::Never, [false, false, false, false]),
    ];
    for (fail_on, expected) in table {
        let got = [Pass, FailOwn, BlockedNotOwn, Unproven].map(|v| fail_on.fails(v));
        assert_eq!(got, expected, "{fail_on:?}");
    }
}

#[test]
fn repository_text_stays_literal_in_markdown() {
    assert_eq!(code("unit::adds"), "`unit::adds`");
    assert_eq!(code("a `b` c"), "``a `b` c``");
    assert_eq!(code("`edge`"), "`` `edge` ``");
    assert_eq!(code("two\nlines"), "`two lines`");
    assert_eq!(
        code_block("```\n<img src=x>\n```"),
        "````text\n```\n<img src=x>\n```\n````"
    );
    assert_eq!(
        escape("ping @octocat *now* <b>x</b>\nnext"),
        "ping @\u{200b}octocat \\*now\\* \\<b\\>x\\<\\/b\\> next"
    );
}
