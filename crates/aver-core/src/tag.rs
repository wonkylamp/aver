//! Criterion tags in test names (ARCHITECTURE.md §1.9.2): how a test says
//! which criteria it proves.
//!
//! Two forms, so every runner can carry one:
//!
//! - the **bracket form** `[crit:<id>]`, anywhere in the name: a Jest,
//!   Vitest, Mocha or node:test title, a Go subtest (`t.Run("[crit:neg]
//!   negates", …)`), a pytest parametrize id (`ids=["crit:neg"]` reports
//!   as `test_negate[crit:neg]`);
//! - the **identifier form** `crit_<id>`, with each `-` and `.` of the id
//!   written `_`, for names that cannot hold brackets: a Rust test function
//!   or module (`fn negates_crit_neg()`, `mod crit_neg { … }`), a Go or
//!   Python function name. It counts as a whole word anywhere in the name,
//!   as a prefix (`crit_neg_negates_zero`) or a suffix
//!   (`negates_crit_neg`): the character before `crit_` is not a letter or
//!   digit, and the one after the id is not a letter or digit. Where several
//!   of a task's criteria fit at one place, the longest wins, so
//!   `crit_neg_two` carries `neg-two` when the task has it and `neg`
//!   otherwise. When two of a task's criteria share an identifier form
//!   (`a-b` and `a_b`), neither gets one: only their bracket forms count.
//!
//! Tags are matched only against the task's own criterion ids.

use crate::CriterionId;

/// The bracket form of `id`'s tag.
pub fn bracket(id: &CriterionId) -> String {
    format!("[crit:{id}]")
}

/// The identifier form of `id`'s tag, whether or not a task can use it.
pub fn ident(id: &CriterionId) -> String {
    let mut out = String::from("crit_");
    out.extend(
        id.as_str()
            .chars()
            .map(|c| if matches!(c, '-' | '.') { '_' } else { c }),
    );
    out
}

#[derive(Debug, Clone)]
struct Tag {
    id: CriterionId,
    bracket: String,
    ident: String,
    /// No other criterion of the task has the same identifier form.
    unique: bool,
}

/// The tags of one task's criteria.
#[derive(Debug, Clone)]
pub struct Tags {
    tags: Vec<Tag>,
}

impl Tags {
    pub fn new<'a>(ids: impl IntoIterator<Item = &'a CriterionId>) -> Self {
        let ids: Vec<&CriterionId> = ids.into_iter().collect();
        let idents: Vec<String> = ids.iter().map(|id| ident(id)).collect();
        let tags = ids
            .iter()
            .zip(&idents)
            .map(|(id, word)| Tag {
                id: (*id).clone(),
                bracket: bracket(id),
                ident: word.clone(),
                unique: idents.iter().filter(|w| *w == word).count() == 1,
            })
            .collect();
        Self { tags }
    }

    /// Whether `text` (a test's name, or a test file's source) carries
    /// `id`'s tag. False for an id that is not one of the task's.
    pub fn carries(&self, text: &str, id: &CriterionId) -> bool {
        self.carried(text).any(|c| c == id)
    }

    /// The criteria whose tag `text` carries, in the order given.
    pub fn carried<'s>(&'s self, text: &'s str) -> impl Iterator<Item = &'s CriterionId> + 's {
        let words = self.words(text);
        self.tags
            .iter()
            .enumerate()
            .filter(move |(i, tag)| text.contains(tag.bracket.as_str()) || words.contains(i))
            .map(|(_, tag)| &tag.id)
    }

    /// The criteria whose identifier form `text` holds as a whole word, by
    /// index. At each `crit_` with no letter or digit before it, the longest
    /// form there followed by anything but a letter or digit wins, unless
    /// two criteria share that form.
    fn words(&self, text: &str) -> Vec<usize> {
        let bytes = text.as_bytes();
        text.match_indices("crit_")
            .filter(|(at, _)| {
                !at.checked_sub(1)
                    .is_some_and(|i| bytes[i].is_ascii_alphanumeric())
            })
            .filter_map(|(at, _)| {
                let rest = &text[at..];
                let (i, tag) = self
                    .tags
                    .iter()
                    .enumerate()
                    .filter(|(_, tag)| {
                        rest.starts_with(&tag.ident)
                            && !rest
                                .as_bytes()
                                .get(tag.ident.len())
                                .is_some_and(|b| b.is_ascii_alphanumeric())
                    })
                    .max_by_key(|(_, tag)| tag.ident.len())?;
                tag.unique.then_some(i)
            })
            .collect()
    }

    /// The identifier form `id` uses in this task, if it has one.
    pub fn ident_of(&self, id: &CriterionId) -> Option<&str> {
        self.tags
            .iter()
            .find(|tag| &tag.id == id && tag.unique)
            .map(|tag| tag.ident.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> CriterionId {
        s.parse().unwrap()
    }

    #[test]
    fn both_forms_count_and_the_identifier_form_only_as_a_whole_word() {
        let (neg, neg_two, dot) = (id("neg"), id("neg-two"), id("api.v2"));
        let tags = Tags::new([&neg, &neg_two, &dot]);
        let yes = |name: &str, c: &CriterionId| assert!(tags.carries(name, c), "{name} {c}");
        let no = |name: &str, c: &CriterionId| assert!(!tags.carries(name, c), "{name} {c}");
        yes("calc > [crit:neg] negates", &neg);
        yes("calc tests/negate.rs > crit_neg", &neg);
        yes("calc src/lib.rs > tests::negates_crit_neg", &neg);
        yes("calc tests/a.rs > crit_neg::negates", &neg);
        yes("example.com/calc > TestNegate/crit_neg", &neg);
        yes("test_negate_crit_neg", &neg);
        yes("x > crit_neg_two", &neg_two);
        yes("x > crit_api_v2::works", &dot);
        yes("x > crit_api_v2_works", &dot);
        // The longest of the task's forms wins where several fit.
        no("x > crit_neg_two", &neg);
        yes("x > crit_neg_twofold", &neg);
        // Not a whole word: letters or digits glued to it.
        no("x > crit_negative", &neg);
        no("x > discrit_neg", &neg);
        no("x > crit_neg2", &neg);
        no("x > [crit:negative]", &neg);
        assert_eq!(
            tags.carried("crit_neg [crit:api.v2]").collect::<Vec<_>>(),
            [&neg, &dot]
        );
        // Not one of the task's criteria: no tag at all.
        no("[crit:other] crit_other", &id("other"));
    }

    #[test]
    fn criteria_sharing_an_identifier_form_keep_only_their_brackets() {
        let (dash, under) = (id("a-b"), id("a_b"));
        let tags = Tags::new([&dash, &under]);
        assert_eq!(tags.ident_of(&dash), None);
        assert!(!tags.carries("crit_a_b", &dash));
        assert!(!tags.carries("crit_a_b", &under));
        assert!(tags.carries("[crit:a-b]", &dash));
        assert!(!tags.carries("[crit:a-b]", &under));
        assert_eq!(Tags::new([&dash]).ident_of(&dash), Some("crit_a_b"));
        // A shorter criterion does not inherit the shared form.
        let a = id("a");
        let tags = Tags::new([&dash, &under, &a]);
        assert!(!tags.carries("crit_a_b", &a));
        assert!(tags.carries("crit_a_c", &a));
    }

    #[test]
    fn the_identifier_form_counts_as_a_prefix_or_a_suffix_anywhere_in_the_name() {
        let foo = id("foo");
        let tags = Tags::new([&foo]);
        for name in [
            "crit_foo_handles_negative_zero",
            "handles_negative_zero_crit_foo",
            "calc tests/a.rs > crit_foo_handles_zero",
            "calc src/lib.rs > tests::handles_zero_crit_foo",
            "pkg > TestDivide/crit_foo_by_zero",
            "tests/test_calc.py > test_crit_foo_divides",
            "crit_foo",
        ] {
            assert!(tags.carries(name, &foo), "{name}");
        }
        for name in [
            "crit_food_safety",
            "crit_foo2",
            "xcrit_foo",
            "Crit_foo",
            "crit_fo",
        ] {
            assert!(!tags.carries(name, &foo), "{name}");
        }
    }
}
