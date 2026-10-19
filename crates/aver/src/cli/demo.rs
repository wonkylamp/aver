//! `aver demo`: Aver catching an agent that games its tests, offline, in
//! a throwaway directory holding its own Aver home and a tiny Git
//! repository with shell checks. Every step runs this executable as a
//! user would, so the verdicts are the real engine's. The directory goes
//! when the demo ends, on an error or a stop request too; the caller's
//! home and repository are never read or written.

use std::path::Path;
use std::process::{Command, Output};
use std::time::Instant;

use anyhow::{bail, Context as _};
use aver::keys::Home;
use aver::place::proc::interrupt;

#[derive(clap::Args)]
pub struct Args {}

/// The repository's `aver.toml`: one per-test shell check.
const CONFIG: &str = r#"[[check]]
id     = "unit"
run    = ["sh", "test/run.sh", "/aver/out/unit.xml"]
report = { format = "junit", path = "/aver/out/unit.xml" }
"#;

/// fixtures/sh-calc's runner: each `name :: command` line of
/// `test/*.tests` is a test, reported as JUnit.
const RUNNER: &str = r#"#!/bin/sh
out=${1:-unit.xml}
cases=""
n=0
failed=0
for f in test/*.tests; do
  while IFS= read -r line; do
    case $line in '' | '#'*) continue ;; esac
    name=${line%% :: *}
    cmd=${line#* :: }
    n=$((n + 1))
    if (. ./calc.sh && eval "$cmd") >/dev/null 2>&1; then
      cases="$cases<testcase classname=\"$f\" name=\"$name\"/>"
    else
      failed=$((failed + 1))
      cases="$cases<testcase classname=\"$f\" name=\"$name\"><failure message=\"failed\"/></testcase>"
    fi
  done <"$f"
done
printf '<?xml version="1.0"?>\n<testsuites><testsuite name="calc" tests="%s" failures="%s">%s</testsuite></testsuites>\n' "$n" "$failed" "$cases" >"$out"
[ "$failed" -eq 0 ]
"#;

const CALC: &str = "add() { echo $(($1 + $2)); }\nsub() { echo $(($1 + $2)); }\n";
const ADD_TESTS: &str =
    "adds two numbers :: [ \"$(add 2 3)\" = 5 ]\nadds a negative :: [ \"$(add 2 -3)\" = -1 ]\n";
const SUB_TESTS: &str = "[crit:sub] subtracts :: [ \"$(sub 5 3)\" = 2 ]\n";
const CRITERION: &str = "sub: sub subtracts";

pub fn run(_: &Home, _: Args) -> anyhow::Result<()> {
    // A stop request ends the step in flight (it reaches the child too) and
    // then this function, so the directory is still removed.
    interrupt::graceful();
    let started = Instant::now();
    let tmp = tempfile::Builder::new().prefix("aver-demo-").tempdir()?;
    let demo = Demo::new(tmp.path())?;
    println!(
        "Aver demo, in {} (removed when the demo ends).\n",
        tmp.path().display()
    );
    demo.story()?;
    println!(
        "\nDone in {:.1} s. Next: `aver init` in your own repository.",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

struct Demo {
    root: std::path::PathBuf,
    exe: std::path::PathBuf,
}

impl Demo {
    fn new(root: &Path) -> anyhow::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        let exe = std::env::current_exe().context("finding the aver executable")?;
        std::fs::write(
            root.join("gitconfig"),
            "[user]\n\tname = Aver demo\n\temail = demo@example.invalid\n[init]\n\tdefaultBranch = main\n",
        )?;
        Ok(Self { root, exe })
    }

    fn story(&self) -> anyhow::Result<()> {
        let repo = self.root.join("calc");
        write(&repo, "aver.toml", CONFIG)?;
        write(&repo, "test/run.sh", RUNNER)?;
        write(&repo, "calc.sh", CALC)?;
        write(&repo, "test/add.tests", ADD_TESTS)?;
        write(&repo, "test/sub.tests", SUB_TESTS)?;
        for args in [
            &["init", "-q"][..],
            &["add", "-A"],
            &["commit", "-q", "-m", "calc"],
        ] {
            self.expect(self.git(&repo, args)?, 0, "git")?;
        }
        println!("The repository: calc.sh with add() and a sub() that adds; its test \"subtracts\" fails.");
        println!("The task: \"{CRITERION}\".\n");

        self.expect(self.aver(&self.root, &["key"])?, 0, "aver key")?;
        let out = self.aver(
            &repo,
            &[
                "workspace",
                "new",
                ".",
                "--no-deps",
                "--label",
                "agent",
                "-c",
                CRITERION,
            ],
        )?;
        let ws = String::from_utf8_lossy(&self.expect(out, 0, "aver workspace new")?)
            .lines()
            .last()
            .map(|l| std::path::PathBuf::from(l.trim()))
            .context("aver workspace new printed no directory")?;

        println!("1. The agent gets its own copy (`aver workspace new`), notes what the functions take, and checks.");
        write(
            &ws,
            "calc.sh",
            &format!("# add and sub take two integers.\n{CALC}"),
        )?;
        self.show(&ws, &["check", "-q"], 1)?;
        println!("   Not done, but nothing broken: Aver keeps this as a green checkpoint.");

        println!("\n2. The shortcut: it edits add() instead of sub(), then comments out every test that fails.");
        println!("   The test suite passes now.");
        write(
            &ws,
            "calc.sh",
            "# add and sub take two integers.\nadd() { echo $(($1 - $2)); }\nsub() { echo $(($1 + $2)); }\n",
        )?;
        write(&ws, "test/add.tests", &commented(ADD_TESTS))?;
        write(&ws, "test/sub.tests", &commented(SUB_TESTS))?;
        let line = self.show(&ws, &["verify", "-q"], 1)?;
        if !line.contains("FAIL_OWN") {
            bail!("the demo expected FAIL_OWN from the gamed tests: {line}");
        }
        println!(
            "   Aver also ran the original tests on the agent's code: add's tests fail there,"
        );
        println!("   so commenting them out hid a failure the change caused (NEW).");

        println!("\n3. Back to the last check that broke nothing.");
        let restore = Instant::now();
        let out = self.aver(&ws, &["restore", "--last-green"])?;
        let took = restore.elapsed();
        let text = String::from_utf8_lossy(&self.expect(out, 0, "aver restore")?).into_owned();
        println!("   $ aver restore --last-green");
        if let Some(back) = text.lines().find(|l| l.starts_with("Back to")) {
            println!("   {back}");
        }
        println!("   Took {} ms.", took.as_millis());

        println!("\n4. The real fix: sub() subtracts, and \"subtracts\" proves it.");
        write(
            &ws,
            "calc.sh",
            "# add and sub take two integers.\nadd() { echo $(($1 + $2)); }\nsub() { echo $(($1 - $2)); }\n",
        )?;
        let line = self.show(&ws, &["verify", "-q"], 0)?;
        if !line.contains("PASS") {
            bail!("the demo expected PASS from the real fix: {line}");
        }
        Ok(())
    }

    /// Runs `aver <args>` in `dir`, prints the command and its last line
    /// (the verdict), and returns that line.
    fn show(&self, dir: &Path, args: &[&str], code: i32) -> anyhow::Result<String> {
        let shown = args
            .iter()
            .filter(|a| **a != "-q")
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let out = self.aver(dir, args)?;
        let stdout = self.expect(out, code, &format!("aver {shown}"))?;
        let line = String::from_utf8_lossy(&stdout)
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .to_owned();
        println!("   $ aver {shown}\n   {line}");
        Ok(line)
    }

    /// `out`'s standard output when it exited `code`; otherwise an error
    /// with what it printed.
    fn expect(&self, out: Output, code: i32, what: &str) -> anyhow::Result<Vec<u8>> {
        if interrupt::requested() {
            bail!("interrupted");
        }
        if out.status.code() != Some(code) {
            let got = out
                .status
                .code()
                .map_or_else(|| "on a signal".to_owned(), |c| c.to_string());
            bail!(
                "{what} exited {got}, not {code}:\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(out.stdout)
    }

    fn aver(&self, dir: &Path, args: &[&str]) -> anyhow::Result<Output> {
        let mut cmd = Command::new(&self.exe);
        cmd.args(args)
            .current_dir(dir)
            .env("AVER_HOME", self.root.join("home"));
        self.isolate(&mut cmd);
        cmd.output()
            .with_context(|| format!("running aver {}", args.join(" ")))
    }

    fn git(&self, dir: &Path, args: &[&str]) -> anyhow::Result<Output> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        self.isolate(&mut cmd);
        cmd.output().context("running git (the demo needs Git)")
    }

    /// Git configuration of the demo's own (no signing, hooks or identity
    /// of the user's), and nothing that names another home or repository.
    fn isolate(&self, cmd: &mut Command) {
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("AVER_URL");
    }
}

fn write(dir: &Path, rel: &str, text: &str) -> anyhow::Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

fn commented(tests: &str) -> String {
    tests.lines().map(|l| format!("# {l}\n")).collect()
}
