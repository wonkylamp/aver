//! What a failing setup command reported, one finding per line (ARCHITECTURE.md
//! §1.9.3, setup findings): a declared `mypy` and the Go preset's
//! `go vet` run as setup commands, so without this their failure is only
//! the check's `(exit)` entry, and a base whose type check already fails
//! hides every finding a change adds behind one `PREEXISTING` entry.
//!
//! The sandbox wrapper prints `[aver] setup command <i> of <n> exited with
//! <s>: <step>` after each failing setup command's output (§2.4.1). The
//! lines before such a marker (back to the previous one) are read for
//! findings of the form `<file>.(py|pyi|go):<line>[:<col>]: <message>`;
//! for mypy only `error:` lines count, not its `note:` lines. A finding is
//! keyed by its file and message, not its line, which moves whenever code
//! above it changes; the same finding twice in one run is two keys, the
//! second suffixed ` (2)`. Lines after the last marker are the check's own
//! output and are not read.

use std::collections::BTreeMap;

/// The marker that ends a failing setup command's output.
const MARKER: &str = "[aver] setup command ";

/// The longest message part of a finding's key.
const MAX_MESSAGE: usize = 300;

/// The findings of a run's failing setup commands, or `None` when the
/// output may be cut (it is as long as `budget`, the run's output limit, so
/// a finding could have been dropped from its middle).
pub fn setup_findings(output: &str, budget: usize) -> Option<Vec<String>> {
    if output.len() >= budget {
        return None;
    }
    let mut out = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut segment: Vec<&str> = Vec::new();
    for line in output.lines() {
        if is_step_marker(line) {
            for l in segment.drain(..) {
                if let Some(key) = finding(l) {
                    let n = seen.entry(key.clone()).or_default();
                    *n += 1;
                    out.push(if *n == 1 { key } else { format!("{key} ({n})") });
                }
            }
            continue;
        }
        segment.push(line);
    }
    Some(out)
}

/// Whether a setup command of the run failed: the wrapper's marker after
/// it is in the output.
pub fn setup_failed(output: &str) -> bool {
    output.lines().any(is_step_marker)
}

/// `[aver] setup command <i> of <n> exited with <s>: <step>`, not the
/// closing line the wrapper prints without the step.
fn is_step_marker(line: &str) -> bool {
    line.strip_prefix(MARKER)
        .and_then(|rest| rest.split_once(" exited with "))
        .is_some_and(|(_, rest)| {
            rest.split_once(": ")
                .is_some_and(|(code, _)| code.parse::<i32>().is_ok())
        })
}

/// The key of a finding line: `<file>: <message>`.
fn finding(line: &str) -> Option<String> {
    let line = line
        .trim_start()
        .strip_prefix("vet: ")
        .unwrap_or(line.trim_start());
    let (file, rest) = line.split_once(':')?;
    let python = file.ends_with(".py") || file.ends_with(".pyi");
    if !(python || file.ends_with(".go")) || file.contains(char::is_whitespace) {
        return None;
    }
    let (num, rest) = rest.split_once(':')?;
    num.parse::<u32>().ok()?;
    // An optional column.
    let rest = match rest.split_once(':') {
        Some((col, after)) if col.parse::<u32>().is_ok() => after,
        _ => rest,
    };
    let message = rest.trim();
    if message.is_empty() || (python && !message.starts_with("error:")) {
        return None;
    }
    let file = file.strip_prefix("./").unwrap_or(file);
    let message: String = message.chars().take(MAX_MESSAGE).collect();
    Some(format!("{file}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mypy_and_go_vet_findings_without_line_numbers() {
        let output = "src/calc/ops.py:3: error: Incompatible return value type (got \"str\", expected \"int\")  [return-value]\nsrc/calc/ops.py:9: note: See https://mypy.rtfd.io\nFound 1 error in 1 file (checked 3 source files)\n[aver] setup command 1 of 2 exited with 1: 'python' '-m' 'mypy' '.'\n# example.com/calc\n./calc.go:10:2: fmt.Printf format %d has arg s of wrong type string\nvet: ./other.go:1:1: expected 'package', found 'EOF'\n[aver] setup command 2 of 2 exited with 1: 'go' 'vet' './...'\ncalc.go:4:1: not a finding, the check's own output\n[aver] setup command 1 of 2 exited with 1\n";
        assert_eq!(
            setup_findings(output, 65_536).unwrap(),
            [
                "src/calc/ops.py: error: Incompatible return value type (got \"str\", expected \"int\")  [return-value]",
                "calc.go: fmt.Printf format %d has arg s of wrong type string",
                "other.go: expected 'package', found 'EOF'",
            ]
        );
    }

    #[test]
    fn the_same_finding_twice_is_two_and_cut_output_is_unknown() {
        let output =
            "a.py:1: error: x\na.py:7: error: x\n[aver] setup command 1 of 1 exited with 1: mypy\n";
        assert_eq!(
            setup_findings(output, 1024).unwrap(),
            ["a.py: error: x", "a.py: error: x (2)"]
        );
        assert_eq!(setup_findings(output, output.len()), None);
        assert_eq!(setup_findings("ok\n", 1024).unwrap(), Vec::<String>::new());
    }
}
