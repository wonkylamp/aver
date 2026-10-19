//! What a verdict covers (§1.9.5): `aver_core::scope::Scope`.

mod common;

use aver_core::scope::{NarrowSide, Narrowing, Reuse, Scope, Weak, WeakKind, MAX_NAMES};
use aver_core::{Level, Network, SkipReason, Skipped};
use common::*;

/// The v3 sample: `typecheck` and `unit`, natively, network off.
fn v3() -> aver_core::ReceiptBody {
    sample_receipt_v3().body
}

#[test]
fn lines_of_the_v3_sample_crit_l1_golden() {
    let scope = Scope::of(&v3(), &[], None);
    assert_eq!(
        scope.lines(),
        [
            "Ran 2 of 2 checks, all per test.",
            "Where: natively on this machine, as your user, network off (best effort).",
        ]
    );
    assert!(scope.compact());
    assert!(scope.names().is_empty());
    assert_eq!(scope.suffix(), "");
}

#[test]
fn names_list_at_most_three_then_count_crit_l1_fence() {
    let mut body = v3();
    let skipped: Vec<Skipped> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|c| Skipped {
            check: p(c),
            reason: SkipReason::NotAffected,
        })
        .collect();
    body.skipped = Some(aver_core::Set::new(skipped).unwrap());
    let scope = Scope::of(&body, &[], None);
    assert_eq!(MAX_NAMES, 3);
    assert_eq!(
        scope.names(),
        ["ran: typecheck, unit", "skipped: a, b, c, +2 more"]
    );
    // Lines are Aver's words: no check name appears in them.
    for line in scope.lines() {
        for name in ["typecheck", "unit", "a,"] {
            assert!(!line.contains(name), "{line}");
        }
    }
}

#[test]
fn each_material_limit_crit_l1_suffix() {
    let mut body = v3();
    body.levels.as_mut().unwrap().insert(p("lint"), Level::Exit);
    body.provenance.as_mut().unwrap().network = Network::On;
    let weak = [
        Weak {
            criterion: p("neg"),
            kind: WeakKind::MissingModule,
        },
        Weak {
            criterion: p("api"),
            kind: WeakKind::NewApi,
        },
    ];
    let older = Reuse {
        runs: 9,
        reused: 4,
        older_verifier: true,
    };
    let scope = Scope::of(&body, &weak, Some(older));
    assert_eq!(
        scope.suffix(),
        "; limits: 1 check by exit code, 1 weak proof, network on, runs cached by an older verifier"
    );
    assert!(scope
        .lines()
        .last()
        .unwrap()
        .ends_with("; 4 of 9 runs reused from the cache, some cached by an older verifier."));
    // This verifier's own cache is not a limit, nor is a skipped check.
    let same = Reuse {
        older_verifier: false,
        ..older
    };
    assert_eq!(Scope::of(&v3(), &[], Some(same)).suffix(), "");
}

/// Narrowed sides are said on the where line: the section stays two lines,
/// names nothing and adds no limit, since the receipt is the full runs'.
#[test]
fn narrowed_sides_on_the_where_line() {
    let scope = Scope::of(&v3(), &[], None).with_narrowed(vec![
        Narrowing {
            side: NarrowSide::Transplant,
            ran: 2,
            of: 14,
        },
        Narrowing {
            side: NarrowSide::HeadTests,
            ran: 1,
            of: 14,
        },
    ]);
    assert_eq!(
        scope.lines(),
        [
            "Ran 2 of 2 checks, all per test.",
            "Where: natively on this machine, as your user, network off (best effort); the transplant ran 2 of 14 test targets, the rest read from the original code's run; your tests' run ran 1 of 14 test targets, the rest read from your code's run.",
        ]
    );
    assert!(scope.compact());
    assert_eq!(scope.suffix(), "");
}

/// A receipt before v3 has no levels: the checks come from its entries,
/// without judging, protected or lease entries.
#[test]
fn v2_checks_from_entries_crit_l1_golden() {
    let scope = Scope::of(&sample_receipt_v2().body, &[], None);
    assert_eq!(scope.lines()[0], "Ran 2 of 2 checks.");
    assert_eq!(
        scope.lines()[1],
        "Where: in a non-native executor (the scripted test executor; before Aver 0.4, the Docker sandbox), network off."
    );
}
