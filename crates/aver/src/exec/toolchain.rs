//! What a run's output says when no test could run (ARCHITECTURE.md
//! §1.9.1, §1.9.3): a toolchain that cannot start the tests at all (a Go
//! version or module error, a Rust toolchain that is not installed, no
//! pytest, a Node option or file extension Node does not know), and the
//! compiler errors of a build that failed.
//!
//! A toolchain error is about the machine or the project's setup, never
//! about the change, so the reports name the line and the fix instead of a
//! "malformed report". A build error is the code's: on the transplant
//! (the original code with the change's tests) an unresolved name the
//! change added is what a test of a new API does there, so it still proves,
//! weakly (§1.9.4, `build_error`).
//!
//! Every line returned here is sandbox output: repository data.

/// A toolchain error: the line that says it, and what fixes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub line: String,
    pub fix: &'static str,
}

impl std::fmt::Display for Toolchain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (fix: {})", self.line, self.fix)
    }
}

/// Most characters of a line kept.
const MAX_LINE: usize = 300;

/// `(pattern, fix)`: a line containing the pattern is a toolchain error.
const SIGNS: &[(&str, &str)] = &[
    (
        "go.mod requires go >=",
        "install the Go version go.mod asks for, or lower its go line",
    ),
    (
        "requires go >=",
        "install the Go version go.mod asks for, or lower its go line",
    ),
    (
        "missing go.sum entry",
        "run `go mod tidy` and commit go.sum",
    ),
    (
        "updates to go.mod needed",
        "run `go mod tidy` and commit go.mod and go.sum",
    ),
    ("errors parsing go.mod", "fix the go.mod syntax"),
    (
        "go.mod file not found",
        "run the check where go.mod is (set its cwd), or `go mod init`",
    ),
    (
        "cannot find main module",
        "run the check where go.mod is (set its cwd), or `go mod init`",
    ),
    (
        "go: downloading go",
        "install the Go version go.mod asks for (the sandbox cannot download one)",
    ),
    (
        "is not installed",
        "install the toolchain rust-toolchain.toml names (`rustup toolchain install`), or remove the pin",
    ),
    (
        "could not find `Cargo.toml`",
        "run the check where Cargo.toml is (set its cwd)",
    ),
    (
        "cannot be built because it requires rustc",
        "install the Rust version Cargo.toml's rust-version asks for, or lower it",
    ),
    (
        "is not supported by the following packages",
        "install the Rust version Cargo.toml's rust-version asks for, or lower it",
    ),
    (
        "Missing script: \"test\"",
        "add a \"test\" script to package.json, or declare the check's command in aver.toml",
    ),
    (
        "No module named pytest",
        "add pytest to the project's dependencies (or its dev group) so the environment installs it",
    ),
    (
        "No module named 'pytest'",
        "add pytest to the project's dependencies (or its dev group) so the environment installs it",
    ),
    (
        "bad option:",
        "use a Node version that knows this option, or drop it from the check",
    ),
    (
        "is not allowed in NODE_OPTIONS",
        "pass the option on the command line instead of NODE_OPTIONS",
    ),
    (
        "ERR_UNKNOWN_FILE_EXTENSION",
        "run TypeScript with Node 22.18 or later (type stripping) or a loader such as tsx",
    ),
];

/// The first toolchain error in `output`, if any, else the first command a
/// shell could not find ([`missing_command`]).
pub fn classify(output: &str) -> Option<Toolchain> {
    output
        .lines()
        .map(str::trim)
        .find_map(|line| {
            let (_, fix) = SIGNS.iter().find(|(sign, _)| line.contains(sign))?;
            // `is not installed` is rustup's only when it names a toolchain.
            if line.contains("is not installed") && !line.contains("toolchain") {
                return None;
            }
            Some(Toolchain {
                line: cut(line),
                fix,
            })
        })
        .or_else(|| missing_command(output))
}

/// The fix of a [`missing_command`] line.
pub const MISSING_COMMAND_FIX: &str = "the command is not installed here, usually because the checkout's dependencies are not: install them before Aver runs (`npm install`, `pnpm install`, `yarn install`, `pip install`, ...), or put the command on PATH";

/// The first line in `output` where a shell or `env` says a command does
/// not exist (exit 127): bash's `sh: mocha: command not found` and
/// `run.sh: line 2: mocha: command not found`, dash's
/// `sh: 1: mocha: not found` and `run.sh: 2: mocha: not found`,
/// `zsh: command not found: mocha`, `env: 'node': No such file or
/// directory`. A test runner that never started ran no test, so its check
/// gave no evidence on that side.
pub fn missing_command(output: &str) -> Option<Toolchain> {
    output.lines().map(str::trim).find_map(|line| {
        let parts: Vec<&str> = line.split(": ").collect();
        let program = parts.first()?.rsplit('/').next()?;
        let shell = matches!(program, "sh" | "bash" | "dash" | "zsh" | "ksh");
        // dash names the script's line by its number alone.
        let numbered = parts
            .get(1)
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        let found = match parts.as_slice() {
            [_, "command not found", name] if shell => Some(*name),
            [_, .., name, "command not found"] => Some(*name),
            [_, .., name, "not found"] if shell || numbered => Some(*name),
            [_, name, "No such file or directory"] if program == "env" => Some(*name),
            _ => None,
        };
        found.filter(|n| !n.trim().is_empty()).map(|_| Toolchain {
            line: cut(line),
            fix: MISSING_COMMAND_FIX,
        })
    })
}

/// The compiler errors in a failed build's output, in order, at most
/// `max`: rustc's `error[E…]` and cargo's `error: …` lines, Go's
/// `file.go:line:col: …` lines, TypeScript's `error TS…` lines, and Python's
/// `ImportError` and `ModuleNotFoundError` lines.
pub fn build_errors(output: &str, max: usize) -> Vec<String> {
    let mut found = Vec::new();
    for line in output.lines().map(str::trim) {
        if found.len() >= max {
            break;
        }
        let rust = (line.starts_with("error[E") || line.starts_with("error: "))
            && !line.starts_with("error: test failed")
            && !line.starts_with("error: could not compile")
            && !line.starts_with("error: aborting");
        let go = go_error(line);
        let ts = line.contains("error TS");
        let python = line.starts_with("ImportError") || line.starts_with("ModuleNotFoundError");
        if rust || go || ts || python {
            found.push(cut(line));
        }
    }
    found
}

/// A Go compiler error line: `[./]path.go:line:col: message`.
fn go_error(line: &str) -> bool {
    let Some((path, rest)) = line.split_once(".go:") else {
        return false;
    };
    let mut parts = rest.splitn(3, ':');
    !path.contains(' ')
        && parts
            .next()
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && parts
            .next()
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && parts.next().is_some_and(|m| m.starts_with(' '))
}

/// Whether a build error only says a name does not resolve: what a test of
/// an API the change adds gets on the original code. Rust E0425, E0432,
/// E0433, E0412 and E0599; Go `undefined:`; TypeScript TS2305, TS2307,
/// TS2339 and TS2724; Python `ImportError` and `ModuleNotFoundError`.
pub fn unresolved_name(error: &str) -> bool {
    const SIGNS: &[&str] = &[
        "error[E0425]",
        "error[E0432]",
        "error[E0433]",
        "error[E0412]",
        "error[E0599]",
        "undefined: ",
        "error TS2305",
        "error TS2307",
        "error TS2339",
        "error TS2724",
        "ImportError",
        "ModuleNotFoundError",
    ];
    SIGNS.iter().any(|s| error.contains(s))
}

/// A version the project asks for that the toolchain here is older than
/// (or, for go.mod's `toolchain` line, does not match): what a native run
/// would refuse to start, said before any run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// The project file and line, e.g. `go.mod: go 1.22`.
    pub asks: String,
    /// The tool and its version here, e.g. `go 1.21.5`.
    pub have: String,
    /// Whether the tests cannot run at all (go.mod's `go` line, Cargo's
    /// `rust-version`); go.mod's `toolchain` line only says which Go the
    /// project prefers, and a native run uses the local one
    /// (`GOTOOLCHAIN=local`).
    pub fatal: bool,
    pub fix: &'static str,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.fatal {
            write!(
                f,
                "{} asks for a newer toolchain than {}, so no test can run here",
                self.asks, self.have
            )
        } else {
            write!(
                f,
                "{} is newer than {}; Aver runs the local one (GOTOOLCHAIN=local)",
                self.asks, self.have
            )
        }
    }
}

const GO_FIX: &str = "install the Go version go.mod asks for, or lower its go line";
const RUST_FIX: &str =
    "install the Rust version Cargo.toml's rust-version asks for (`rustup update`), or lower it";

/// The Go and Rust versions the project's root `go.mod` and `Cargo.toml`
/// ask for that `go` and `rustc` here (their `go version` and `rustc
/// --version` output) cannot meet. A tool not given, a file not given or a
/// version that does not parse is no mismatch.
pub fn mismatches(
    go_mod: Option<&str>,
    cargo_toml: Option<&str>,
    go: Option<&str>,
    rustc: Option<&str>,
) -> Vec<Mismatch> {
    let mut out = Vec::new();
    if let (Some(go_mod), Some(have)) = (go_mod, go.and_then(go_version)) {
        for line in go_mod.lines().map(str::trim) {
            let (directive, fatal) = match line.split_once(char::is_whitespace) {
                Some(("go", v)) => (v.trim(), true),
                Some(("toolchain", v)) => (v.trim().trim_start_matches("go"), false),
                _ => continue,
            };
            let Some(wants) = numbers(directive) else {
                continue;
            };
            if older(&have, &wants) {
                out.push(Mismatch {
                    asks: format!("go.mod: {}", cut(line)),
                    have: format!("go {}", join(&have)),
                    fatal,
                    fix: GO_FIX,
                });
            }
        }
    }
    if let (Some(cargo), Some(have)) = (cargo_toml, rustc.and_then(rustc_version)) {
        if let Some(wants) = rust_version(cargo) {
            if numbers(&wants).is_some_and(|w| older(&have, &w)) {
                out.push(Mismatch {
                    asks: format!("Cargo.toml: rust-version = \"{}\"", cut(&wants)),
                    have: format!("rustc {}", join(&have)),
                    fatal: true,
                    fix: RUST_FIX,
                });
            }
        }
    }
    out
}

/// What `go version` and `rustc --version` print here, from `PATH`, both
/// from `cwd` (a directory whose `rust-toolchain.toml` would pick a pinned
/// Rust, or a neutral one). `None` for a tool that is missing or does not
/// answer, and for a Go that fetches the version go.mod asks for: a native
/// run inherits the caller's `GOTOOLCHAIN`, and under its default `auto`
/// any go line can run.
pub fn local_versions(cwd: &std::path::Path) -> (Option<String>, Option<String>) {
    let run = |tool: &str, args: &[&str], local: bool| {
        let mut command = std::process::Command::new(tool);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null());
        if local {
            command.env("GOTOOLCHAIN", "local");
        }
        let out = command.output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let go = run("go", &["env", "GOTOOLCHAIN"], false)
        .filter(|mode| !go_switches(mode))
        .and_then(|_| run("go", &["version"], true));
    (go, run("rustc", &["--version"], true))
}

/// Whether Go, under `go env GOTOOLCHAIN`'s answer, may run a newer
/// toolchain than its own: every mode but `local` may (`auto`,
/// `<name>+auto`, `path`, a named toolchain).
fn go_switches(mode: &str) -> bool {
    mode.trim() != "local"
}

/// `rust-version` of the root package, or of `[workspace.package]` when
/// the package inherits it or there is no package.
fn rust_version(cargo_toml: &str) -> Option<String> {
    let doc: toml::Table = cargo_toml.parse().ok()?;
    let of = |t: Option<&toml::Value>| {
        t.and_then(|t| t.get("rust-version"))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    };
    of(doc.get("package")).or_else(|| of(doc.get("workspace").and_then(|w| w.get("package"))))
}

/// `go version go1.21.5 linux/amd64` → `[1, 21, 5]`.
fn go_version(output: &str) -> Option<Vec<u64>> {
    output
        .split_whitespace()
        .find_map(|w| w.strip_prefix("go").and_then(numbers))
}

/// `rustc 1.97.0 (c980f4866 2026-06-30)` → `[1, 97, 0]`.
fn rustc_version(output: &str) -> Option<Vec<u64>> {
    output.split_whitespace().nth(1).and_then(numbers)
}

/// `1.22`, `1.22.3` or `1.23rc1` as numbers; a pre-release suffix ends the
/// last one.
fn numbers(v: &str) -> Option<Vec<u64>> {
    let parts: Vec<u64> = v
        .split('.')
        .map_while(|p| {
            let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect();
    (parts.len() >= 2).then_some(parts)
}

/// Whether `have` is older than `wants`, missing components being 0.
fn older(have: &[u64], wants: &[u64]) -> bool {
    let n = have.len().max(wants.len());
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    (0..n)
        .map(|i| at(have, i).cmp(&at(wants, i)))
        .find(|o| o.is_ne())
        .is_some_and(|o| o.is_lt())
}

fn join(v: &[u64]) -> String {
    v.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

fn cut(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_that_cannot_find_a_command_is_a_missing_command() {
        for line in [
            "sh: mocha: command not found",
            "/bin/sh: line 1: xo: command not found",
            "check.sh: line 2: calc-check: command not found",
            "check.sh: 2: calc-check: not found",
            "sh: 1: jest: not found",
            "zsh: command not found: vitest",
            "env: 'node': No such file or directory",
        ] {
            let found = classify(&format!("> pkg@1.0.0 test\n> mocha\n\n{line}\n"));
            assert_eq!(
                found.map(|t| (t.line, t.fix)),
                Some((line.to_owned(), MISSING_COMMAND_FIX)),
                "{line}"
            );
        }
        for line in [
            "Error: page not found",
            "expected 'command not found' to equal 'ok'",
            "AssertionError: sh: x: something else",
        ] {
            assert_eq!(classify(line), None, "{line}");
        }
    }

    #[test]
    fn toolchain_errors_name_the_line_and_the_fix() {
        let table: &[(&str, &str)] = &[
            (
                "go: go.mod requires go >= 1.23 (running go 1.21.5; GOTOOLCHAIN=local)",
                "install the Go version",
            ),
            (
                "go: example.com/calc: missing go.sum entry for module providing package golang.org/x/text",
                "go mod tidy",
            ),
            (
                "go: go.mod file not found in current directory or any parent directory; see 'go help modules'",
                "go mod init",
            ),
            (
                "error: toolchain '1.80.0-aarch64-apple-darwin' is not installed",
                "rustup toolchain install",
            ),
            (
                "error: could not find `Cargo.toml` in `/work` or any parent directory",
                "Cargo.toml",
            ),
            ("/usr/bin/python3: No module named pytest", "add pytest"),
            (
                "error: package `calc v0.1.0 (/work)` cannot be built because it requires rustc 1.99 or newer, while the currently active rustc version is 1.97.0",
                "rust-version",
            ),
            (
                "error: rustc 1.97.0 is not supported by the following packages:",
                "rust-version",
            ),
            ("npm error Missing script: \"test\"", "\"test\" script"),
            ("node: bad option: --experimental-strip-types", "Node version"),
            (
                "TypeError [ERR_UNKNOWN_FILE_EXTENSION]: Unknown file extension \".ts\" for /work/test/a.test.ts",
                "tsx",
            ),
        ];
        for (line, fix) in table {
            let found = classify(&format!("some noise\n  {line}\nmore\n"))
                .unwrap_or_else(|| panic!("{line} is a toolchain error"));
            assert_eq!(found.line, *line);
            assert!(found.fix.contains(fix), "{line}: {}", found.fix);
        }
        for not in [
            "error[E0425]: cannot find value `x` in this scope",
            "./calc_test.go:7:9: undefined: Mul",
            "the package left-pad is not installed",
            "AssertionError: 1 !== 2",
        ] {
            assert_eq!(classify(not), None, "{not}");
        }
    }

    #[test]
    fn a_toolchain_older_than_the_project_asks_for_is_a_mismatch_crit_ci_trust_doctor() {
        let go = Some("go version go1.21.5 linux/amd64");
        let found = mismatches(Some("module x\n\ngo 1.22\n"), None, go, None);
        assert_eq!(found.len(), 1);
        assert!(found[0].fatal);
        assert_eq!(found[0].asks, "go.mod: go 1.22");
        assert_eq!(found[0].have, "go 1.21.5");
        assert!(found[0].to_string().contains("no test can run here"));
        assert!(mismatches(Some("go 1.21\n"), None, go, None).is_empty());
        assert!(mismatches(Some("go 1.21.5\n"), None, go, None).is_empty());
        let toolchain = mismatches(Some("go 1.21\ntoolchain go1.23.4\n"), None, go, None);
        assert_eq!(toolchain.len(), 1);
        assert!(!toolchain[0].fatal);

        let rustc = Some("rustc 1.97.0 (c980f4866 2026-06-30)");
        let cargo = "[package]\nname = \"calc\"\nrust-version = \"1.99\"\n";
        let found = mismatches(None, Some(cargo), None, rustc);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].asks, "Cargo.toml: rust-version = \"1.99\"");
        assert!(found[0].fix.contains("rust-version"));
        let workspace = "[workspace.package]\nrust-version = \"1.98.1\"\n[package]\nname = \"a\"\nrust-version.workspace = true\n";
        assert_eq!(mismatches(None, Some(workspace), None, rustc).len(), 1);
        let fine = "[package]\nname = \"calc\"\nrust-version = \"1.80\"\n";
        assert!(mismatches(None, Some(fine), None, rustc).is_empty());
        assert!(mismatches(Some("go 1.99\n"), Some(cargo), None, None).is_empty());
    }

    #[test]
    fn only_a_go_that_stays_local_is_held_to_its_own_version() {
        assert!(!go_switches("local\n"));
        for mode in ["auto\n", "go1.21.5+auto", "path", "local+path", "go1.26.0"] {
            assert!(go_switches(mode), "{mode}");
        }
    }

    #[test]
    fn build_errors_are_read_per_language_and_unresolved_names_told_apart() {
        let rust = "   Compiling calc v0.1.0\nerror[E0425]: cannot find function `mul` in crate `calc`\n --> tests/calc.rs:9:5\nerror: could not compile `calc` (test \"calc\") due to 1 previous error\n";
        assert_eq!(
            build_errors(rust, 5),
            ["error[E0425]: cannot find function `mul` in crate `calc`"]
        );
        let go = "# example.com/calc [example.com/calc.test]\n./calc_test.go:20:13: undefined: Mul\nFAIL\texample.com/calc [build failed]\n";
        assert_eq!(
            build_errors(go, 5),
            ["./calc_test.go:20:13: undefined: Mul"]
        );
        let ts = "test/a.test.ts(1,10): error TS2305: Module '\"../src/a\"' has no exported member 'mul'.";
        assert_eq!(build_errors(ts, 5).len(), 1);
        let py = "ImportError: cannot import name 'mul' from 'calc'";
        assert_eq!(build_errors(py, 5).len(), 1);
        assert!(build_errors("all good\nok  \texample.com/calc 0.1s", 5).is_empty());

        for unresolved in [
            "error[E0425]: cannot find function `mul` in crate `calc`",
            "error[E0432]: unresolved import `calc::mul`",
            "./calc_test.go:20:13: undefined: Mul",
            "test/a.test.ts(1,10): error TS2305: Module has no exported member 'mul'.",
            "ImportError: cannot import name 'mul' from 'calc'",
        ] {
            assert!(unresolved_name(unresolved), "{unresolved}");
        }
        for other in [
            "error[E0308]: mismatched types",
            "./calc.go:3:1: syntax error: non-declaration statement outside function body",
            "error TS2322: Type 'string' is not assignable to type 'number'.",
        ] {
            assert!(!unresolved_name(other), "{other}");
        }
    }
}
