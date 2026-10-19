//! Cross-checking attempts at the end of a run (ARCHITECTURE.md §2.10): each
//! of the top attempts' own tests run against another attempt's code in a
//! dry-run verify, and the tests that come out `NEW` count against it.
//!
//! For the ordered pair (i, j), a workspace opens on j's head, i's added and
//! modified test files are written into it, and the snapshot is verified
//! against j's head as a dry run: nothing is stored and no budget is spent.
//! A test file both added at one path goes in under another name
//! ([`cross_name`]), so j's own copy stays and both run.
//! A test of i's that is `NEW` there passes on nothing j wrote: j's code
//! misses a behaviour i tested. A test that cannot load or build on j's code
//! tests a different API, which says nothing about behaviour, so it is not
//! counted.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aver_core::{Cause, SigningKey, SnapshotId, TaskId};
use serde::{Deserialize, Serialize};

use super::api::{Api, ApiError};
use super::compare::Compared;
use super::state::run_dir;
use crate::keys::Home;

/// The attempts cross-checked: at most this many from the top.
pub const TOP: usize = 3;
/// Wall time for one pair.
pub const PAIR_LIMIT: Duration = Duration::from_secs(120);
/// Wall time for every pair of a run together.
pub const TOTAL_LIMIT: Duration = Duration::from_secs(300);

/// `aver run --cross`: cross-check at the end of a run, or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum CrossMode {
    /// When two or more attempts end in `PASS`, `REVIEW` or `UNPROVEN`.
    #[default]
    Auto,
    Off,
}

/// `runs/<id>/cross.json`: every pair of a run's cross-check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossReport {
    pub pairs: Vec<Pair>,
}

/// One attempt's tests on another attempt's code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    /// The agent (its number in the run) whose tests ran.
    pub tests_of: u32,
    /// The agent whose code they ran on.
    pub code_of: u32,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Ran {
        /// The tests of `tests_of` that fail on `code_of`'s code and pass
        /// on `tests_of`'s own (repository content).
        failed: Vec<String>,
        /// The tests that could not load or build on `code_of`'s code: a
        /// different API, not counted.
        different_api: Vec<String>,
        ms: u64,
    },
    /// Not run, and why (Aver's words).
    Skipped { why: String },
}

impl CrossReport {
    pub fn path(home: &Home, run: &str) -> PathBuf {
        run_dir(home, run).join("cross.json")
    }

    pub fn read(home: &Home, run: &str) -> Option<Self> {
        let bytes = std::fs::read(Self::path(home, run)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Writes `cross.json` (temporary file, then rename).
    pub fn write(&self, home: &Home, run: &str) -> std::io::Result<()> {
        let path = Self::path(home, run);
        std::fs::create_dir_all(path.parent().expect("cross.json has a directory"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).expect("cross report serializes"),
        )?;
        std::fs::rename(tmp, path)
    }

    /// The other agents' tests that fail on agent `code_of`'s code, per
    /// agent whose tests they are.
    pub fn against(&self, code_of: u32) -> Vec<(u32, Vec<String>)> {
        self.pairs
            .iter()
            .filter(|p| p.code_of == code_of)
            .filter_map(|p| match &p.outcome {
                Outcome::Ran { failed, .. } if !failed.is_empty() => {
                    Some((p.tests_of, failed.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// How many distinct tests of the others fail on `code_of`'s code.
    pub fn failed_on(&self, code_of: u32) -> u32 {
        self.against(code_of)
            .iter()
            .flat_map(|(of, tests)| tests.iter().map(move |t| (*of, t)))
            .collect::<BTreeSet<_>>()
            .len() as u32
    }
}

/// The attempts a cross-check compares, best first: the top [`TOP`] in a
/// class a person might accept, when there are at least two.
pub fn eligible(rows: &[Compared]) -> Vec<&Compared> {
    let top: Vec<&Compared> = super::compare::ranked(rows)
        .into_iter()
        .filter(|(r, c)| r.class.acceptable() && c.head.is_some())
        .map(|(_, c)| c)
        .take(TOP)
        .collect();
    if top.len() < 2 {
        Vec::new()
    } else {
        top
    }
}

/// Cross-checks the eligible attempts of a run on `task`, within
/// [`TOTAL_LIMIT`]; pairs past it are skipped and say so.
pub async fn run(api: &Api, admin: &SigningKey, task: &TaskId, rows: &[Compared]) -> CrossReport {
    let top = eligible(rows);
    let started = Instant::now();
    let mut pairs = Vec::new();
    for i in &top {
        for j in &top {
            if i.agent.index == j.agent.index {
                continue;
            }
            let pair = |outcome| Pair {
                tests_of: i.agent.index,
                code_of: j.agent.index,
                outcome,
            };
            if i.test_files.is_empty() {
                pairs.push(pair(Outcome::Skipped {
                    why: format!("#{} added or changed no test files", i.agent.index),
                }));
                continue;
            }
            let files = match placed(i, j) {
                Ok(files) => files,
                Err(shared) => {
                    pairs.push(pair(Outcome::Skipped {
                        why: format!(
                            "both changed the test file {}{}",
                            super::report::one_line_plain(shared[0]),
                            match shared.len() {
                                1 => String::new(),
                                n => format!(" and {} more", n - 1),
                            }
                        ),
                    }));
                    continue;
                }
            };
            let left = TOTAL_LIMIT.saturating_sub(started.elapsed());
            if left.is_zero() {
                pairs.push(pair(Outcome::Skipped {
                    why: format!(
                        "out of time: the cross-check stops after {} s",
                        TOTAL_LIMIT.as_secs()
                    ),
                }));
                continue;
            }
            let limit = left.min(PAIR_LIMIT);
            let t = Instant::now();
            let outcome =
                match tokio::time::timeout(limit, one(api, admin, task, i, j, &files)).await {
                    Ok(Ok((failed, different_api))) => Outcome::Ran {
                        failed,
                        different_api,
                        ms: t.elapsed().as_millis() as u64,
                    },
                    Ok(Err(e)) => Outcome::Skipped {
                        why: format!(
                            "could not run: {}",
                            super::report::one_line_plain(&e.to_string())
                        ),
                    },
                    Err(_) => Outcome::Skipped {
                        why: format!("took longer than {} s", limit.as_secs()),
                    },
                };
            pairs.push(pair(outcome));
        }
    }
    CrossReport { pairs }
}

/// Where each of i's test files goes in j's tree, as `(i's path, path in
/// j's tree)`: its own path, or for a file both added there, its
/// [`cross_name`]. `Err` with the files both changed that the base had, or
/// that no other name fits: those j's tree cannot hold beside its own.
fn placed<'a>(i: &'a Compared, j: &'a Compared) -> Result<Vec<(String, String)>, Vec<&'a String>> {
    let mut files = Vec::new();
    let mut shared = Vec::new();
    for path in &i.test_files {
        if !j.test_files.contains(path) {
            files.push((path.clone(), path.clone()));
            continue;
        }
        let both_added = i.added_tests.contains(path) && j.added_tests.contains(path);
        match cross_name(path).filter(|_| both_added) {
            Some(renamed) if !j.test_files.contains(&renamed) => {
                files.push((path.clone(), renamed));
            }
            _ => shared.push(path),
        }
    }
    if shared.is_empty() {
        Ok(files)
    } else {
        Err(shared)
    }
}

/// Another name for a test file in the same directory that the runners
/// which found it still find: `.aver-cross` before the first dot of the name
/// (`tests/new.test.js` to `tests/new.aver-cross.test.js`, so `*.test.js`
/// still matches), and for Rust, Python and Go, whose module names take no
/// dot, `_aver_cross` before `_test` or the extension (`tests/x.rs` to
/// `tests/x_aver_cross.rs`, `test_x.py` to `test_x_aver_cross.py`,
/// `x_test.go` to `x_aver_cross_test.go`). `None` for a name with no stem.
pub fn cross_name(path: &str) -> Option<String> {
    let (dir, name) = path.rsplit_once('/').map_or(("", path), |(d, n)| (d, n));
    let (stem, rest) = name.split_once('.').unwrap_or((name, ""));
    if stem.is_empty() {
        return None;
    }
    let renamed = if matches!(rest, "rs" | "py" | "go") {
        match stem.strip_suffix("_test") {
            Some(s) => format!("{s}_aver_cross_test.{rest}"),
            None => format!("{stem}_aver_cross.{rest}"),
        }
    } else if rest.is_empty() {
        format!("{stem}.aver-cross")
    } else {
        format!("{stem}.aver-cross.{rest}")
    };
    Some(if dir.is_empty() {
        renamed
    } else {
        format!("{dir}/{renamed}")
    })
}

/// i's tests, placed by [`placed`], on j's code: `(failed, different_api)`.
async fn one(
    api: &Api,
    admin: &SigningKey,
    task: &TaskId,
    i: &Compared,
    j: &Compared,
    placed: &[(String, String)],
) -> Result<(Vec<String>, Vec<String>), ApiError> {
    let (Some(i_head), Some(j_head)) = (i.head, j.head) else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut files = Vec::new();
    for (from, to) in placed {
        files.push((to.clone(), api.read_file(&i_head, from).await?));
    }
    let ws = api.open_workspace(admin, &j_head).await?;
    let verified = async {
        api.edit(admin, &ws, &files).await?;
        let tree: SnapshotId = api.snapshot(admin, &ws).await?;
        api.verify_head_dry(admin, task, &tree, &j_head).await
    }
    .await;
    let _ = api.discard(admin, &ws).await;
    let v = verified?;
    let mut failed = Vec::new();
    let mut different_api = Vec::new();
    for entry in v.receipt.body.checks.iter() {
        if entry.cause != Some(Cause::New) {
            continue;
        }
        let failure = v
            .diagnostics
            .as_ref()
            .and_then(|d| d.failures.iter().find(|f| f.id == entry.id));
        let text = failure
            .map(|f| {
                format!(
                    "{}\n{}",
                    f.message.as_deref().unwrap_or_default(),
                    f.output.as_deref().unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let id = entry.id.to_string();
        let test = match id.split_once("::") {
            Some((_, test)) => test.to_owned(),
            // A whole check failing (a type check, a build) is the API
            // differing, not a behaviour.
            None => {
                different_api.push(id);
                continue;
            }
        };
        if cannot_load(&text) {
            different_api.push(test);
        } else {
            failed.push(test);
        }
    }
    Ok((failed, different_api))
}

/// Whether a failure says the test never reached an assertion: a module,
/// export or name it uses does not exist in this code.
pub fn cannot_load(text: &str) -> bool {
    const SIGNS: &[&str] = &[
        "is not a function",
        "is not a constructor",
        "is not defined",
        "SyntaxError",
        "ImportError",
        "ModuleNotFoundError",
        "cannot find function",
        "cannot find value",
        "unresolved import",
        "error[E0425]",
        "error[E0432]",
        "error[E0433]",
        "error[E0412]",
        "error[E0599]",
        "undefined:",
        "error TS2305",
        "error TS2307",
        "error TS2339",
        "error TS2724",
    ];
    crate::diagnostics::missing_module(text) || SIGNS.iter().any(|s| text.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ran(tests_of: u32, code_of: u32, failed: &[&str]) -> Pair {
        Pair {
            tests_of,
            code_of,
            outcome: Outcome::Ran {
                failed: failed.iter().map(|s| (*s).to_owned()).collect(),
                different_api: Vec::new(),
                ms: 5,
            },
        }
    }

    #[test]
    fn failures_count_once_per_test_and_only_against_the_code_they_ran_on() {
        let report = CrossReport {
            pairs: vec![
                ran(1, 2, &["a", "b"]),
                ran(3, 2, &["a"]),
                ran(2, 1, &[]),
                Pair {
                    tests_of: 1,
                    code_of: 3,
                    outcome: Outcome::Skipped {
                        why: "both changed the test file t".into(),
                    },
                },
            ],
        };
        assert_eq!(report.failed_on(2), 3);
        assert_eq!(report.failed_on(1), 0);
        assert_eq!(report.failed_on(3), 0);
        assert_eq!(report.against(2).len(), 2);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["pairs"][0]["outcome"], "ran");
        assert_eq!(json["pairs"][3]["why"], "both changed the test file t");
        let back: CrossReport = serde_json::from_value(json).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn a_cross_name_keeps_the_runners_pattern() {
        for (from, to) in [
            ("tests/new.test.js", "tests/new.aver-cross.test.js"),
            ("test/neg.test.json", "test/neg.aver-cross.test.json"),
            ("src/a.spec.ts", "src/a.aver-cross.spec.ts"),
            ("tests/x.rs", "tests/x_aver_cross.rs"),
            ("tests/test_x.py", "tests/test_x_aver_cross.py"),
            ("pkg/x_test.py", "pkg/x_aver_cross_test.py"),
            ("calc/calc_test.go", "calc/calc_aver_cross_test.go"),
            ("Makefile", "Makefile.aver-cross"),
        ] {
            assert_eq!(cross_name(from).as_deref(), Some(to), "{from}");
        }
        assert_eq!(cross_name("test/.hidden"), None);
    }

    #[test]
    fn load_and_build_errors_are_a_different_api() {
        assert!(cannot_load(
            "Error [ERR_MODULE_NOT_FOUND]: Cannot find module '/w/src/negate.js'"
        ));
        assert!(cannot_load("TypeError: calc.mul is not a function"));
        assert!(cannot_load("./calc_test.go:9:9: undefined: Mul"));
        assert!(cannot_load(
            "error[E0425]: cannot find function `mul` in this scope"
        ));
        assert!(!cannot_load(
            "AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\n\n-15 !== 15"
        ));
    }
}
