//! Test ids that differ between runs only in a parameter or subtest suffix
//! (REDESIGN.md §4.5): a pytest parameter built from the clock
//! (`test_at[2026-10-17T10:01]`), a Go subtest named after a pointer
//! (`TestX/0xc000123`). The verifier pairs such ids across sides, so a test
//! the head left alone does not read as missing from the head's report.

use std::collections::BTreeMap;

/// A test id's function and suffix, split in its last ` > ` segment: before
/// the first `[` when the segment ends in `]` (pytest's parameters), else
/// before the first `/` (Go's subtests). `None` for an id without either,
/// or with one segment only, which names a file or a command, not a test.
pub fn split(id: &str) -> Option<(&str, &str)> {
    let start = id.rfind(" > ")? + " > ".len();
    let name = &id[start..];
    let cut = if name.ends_with(']') {
        name.find('[')
    } else {
        name.find('/')
    }?;
    (cut > 0).then(|| id.split_at(start + cut))
}

/// Pairs each id of `lost` (one side reported it, the other did not) with
/// the id of `gained` (the reverse) that runs the same function under
/// another suffix: per function, only when both have the same number of
/// ids, matched in sorted order.
pub fn pair<'a>(lost: &[&'a str], gained: &[&'a str]) -> Vec<(&'a str, &'a str)> {
    let by_function = |ids: &[&'a str]| {
        let mut groups: BTreeMap<&'a str, Vec<&'a str>> = BTreeMap::new();
        for id in ids {
            if let Some((function, _)) = split(id) {
                groups.entry(function).or_default().push(id);
            }
        }
        for ids in groups.values_mut() {
            ids.sort_unstable();
        }
        groups
    };
    let gained = by_function(gained);
    by_function(lost)
        .into_iter()
        .filter_map(|(function, lost)| {
            let gained = gained.get(function)?;
            (gained.len() == lost.len()).then(|| lost.into_iter().zip(gained.iter().copied()))
        })
        .flatten()
        .collect()
}
