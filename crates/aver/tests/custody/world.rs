//! A fresh repository from `fixtures/sh-calc` and a fresh `$AVER_HOME`,
//! and the `aver` binary (built with `faults`) run against them. The checks
//! are shell, so nothing here needs Node or Docker.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct World {
    _tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub repo: PathBuf,
    pub faults: PathBuf,
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

pub fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

pub fn text(out: &Output) -> String {
    format!(
        "exit {:?}\n--- stdout\n{}--- stderr\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The change every agent makes: `neg`, and the criterion's test.
pub fn solve(dir: &Path) {
    let calc = dir.join("calc.sh");
    let mut s = fs::read_to_string(&calc).unwrap();
    s.push_str("neg() { echo $((0 - $1)); }\n");
    fs::write(&calc, s).unwrap();
    fs::write(
        dir.join("test/neg.tests"),
        "[crit:neg] negates :: [ \"$(neg 2)\" = -2 ]\n",
    )
    .unwrap();
}

pub const CRITERION: &str = "neg: neg(x) returns -x";
pub const DOUBLE: &str = "dbl: dbl.sh x prints 2x";

/// The scripted agent of a batch: `neg` for a task that negates, `dbl.sh`
/// (a file of its own, so the two merge cleanly) for one that doubles.
const AGENT: &str = r#"cd "$1"
case "$2" in
*negate*)
  printf 'neg() { echo $((0 - $1)); }\n' >> calc.sh
  printf '[crit:neg] negates :: [ "$(neg 2)" = -2 ]\n' > test/neg.tests
  ;;
*double*)
  printf 'echo $(($1 * 2))\n' > dbl.sh
  printf '[crit:dbl] doubles :: [ "$(sh dbl.sh 2)" = 4 ]\n' > test/dbl.tests
  ;;
esac
"#;

/// A stub `gh` for `aver accept --pr`: `pr create` opens pull request 7,
/// and `pr view` finds it open once created. Every call is logged.
const GH: &str = r#"#!/bin/sh
dir=$(dirname "$0")
echo "$@" >> "$dir/gh.log"
case "$1 $2" in
"pr view")
  if [ -f "$dir/gh.created" ]; then
    echo '{"url":"https://github.com/o/r/pull/7","state":"OPEN"}'
  else
    echo 'no pull requests found' >&2
    exit 1
  fi
  ;;
"pr create")
  touch "$dir/gh.created"
  echo https://github.com/o/r/pull/7
  ;;
esac
"#;

impl World {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        copy_dir(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sh-calc"),
            &repo,
        );
        git(&repo, &["init", "--quiet", "--initial-branch=main"]);
        git(&repo, &["config", "user.name", "Test User"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "--quiet", "-m", "sh-calc"]);
        let faults = root.join("faults");
        fs::create_dir_all(&faults).unwrap();
        Self {
            _tmp: tmp,
            home: root.join("home"),
            repo,
            faults,
        }
    }

    /// Runs `aver args` in `cwd` with `fault` as `AVER_FAULT`.
    pub fn aver(&self, cwd: &Path, args: &[&str], fault: Option<&str>) -> Output {
        self.command(Path::new(env!("CARGO_BIN_EXE_aver")), cwd, args, fault)
            .output()
            .unwrap()
    }

    /// `exe args` in `cwd` against this world, with `fault` as
    /// `AVER_FAULT`.
    pub fn command(&self, exe: &Path, cwd: &Path, args: &[&str], fault: Option<&str>) -> Command {
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .current_dir(cwd)
            .env("AVER_HOME", &self.home)
            .env("AVER_FAULT_DIR", &self.faults)
            .env("AVER_APPS_DIR", self.faults.join("no-apps"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("AVER_TEST_HUMAN", "1")
            .env_remove("AVER_URL")
            .env_remove("AVER_FAULT")
            // A test binary that also runs run_support sets it process-wide.
            .env_remove("AVER_SCRIPTED");
        for var in aver::run::accept::AGENT_ENV {
            cmd.env_remove(var);
        }
        let path = format!(
            "{}:{}",
            self.faults.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env("PATH", path);
        if let Some(f) = fault {
            cmd.env("AVER_FAULT", f);
        }
        cmd
    }

    /// [`Self::aver`], asserting it succeeds; returns stdout.
    pub fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.aver(cwd, args, None);
        assert!(out.status.success(), "aver {args:?}: {}", text(&out));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A fresh workspace directory of the repository on the criterion.
    pub fn workspace(&self) -> PathBuf {
        let out = self.ok(&self.repo, &["workspace", "new", "-c", CRITERION]);
        PathBuf::from(out.lines().last().unwrap().trim())
    }

    /// A workspace directory whose `calc.sh` has an edit no sync recorded;
    /// returns the file and what it holds.
    pub fn edited_workspace(&self) -> (PathBuf, Vec<u8>) {
        let file = self.workspace().join("calc.sh");
        let mut bytes = fs::read(&file).unwrap();
        bytes.extend_from_slice(b"# an edit after the last sync\n");
        fs::write(&file, &bytes).unwrap();
        (file, bytes)
    }

    /// A workspace directory of the repository, solved but not verified.
    pub fn solved_workspace(&self) -> PathBuf {
        let dir = self.workspace();
        solve(&dir);
        dir
    }

    /// A workspace directory of the repository whose change verifies PASS.
    pub fn passing_workspace(&self) -> PathBuf {
        let dir = self.solved_workspace();
        let report = self.ok(&dir, &["verify"]);
        assert!(report.starts_with("DONE: PASS"), "{report}");
        dir
    }

    /// An `aver run` of one scripted agent that solves the task; returns
    /// the run's id.
    pub fn finished_run(&self) -> String {
        let agent = self.faults.join("agent.sh");
        fs::write(
            &agent,
            "cd \"$1\"\nprintf 'neg() { echo $((0 - $1)); }\\n' >> calc.sh\nprintf '[crit:neg] negates :: [ \"$(neg 2)\" = -2 ]\\n' > test/neg.tests\n",
        )
        .unwrap();
        let out = self.aver(
            &self.repo,
            &[
                "run",
                "negate numbers",
                "-c",
                CRITERION,
                "--agents",
                "1",
                "--agent-cmd",
                &format!("/bin/sh {} {{dir}}", agent.display()),
                "--no-tui",
                "--no-deps",
            ],
            None,
        );
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        let runs: Vec<_> = fs::read_dir(self.home.join("runs"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(runs.len(), 1, "{runs:?}");
        runs[0].clone()
    }

    /// The `--agent-cmd` of [`Self::batch`]'s agent.
    pub fn agent_cmd(&self) -> String {
        let agent = self.faults.join("batch-agent.sh");
        fs::write(&agent, AGENT).unwrap();
        format!("/bin/sh {} {{dir}} {{prompt}}", agent.display())
    }

    /// `aver run --tasks` of one task per `(title, criterion)`, each solved
    /// by the scripted agent.
    pub fn batch(&self, tasks: &[(&str, &str)]) {
        let file = self.faults.join("tasks.txt");
        let lines: String = tasks
            .iter()
            .map(|(title, crit)| format!("{title}\n  - {crit}\n"))
            .collect();
        fs::write(&file, lines).unwrap();
        let cmd = self.agent_cmd();
        let out = self.aver(
            &self.repo,
            &[
                "run",
                "--tasks",
                &file.display().to_string(),
                "--agent-cmd",
                &cmd,
                "--no-tui",
                "--no-deps",
            ],
            None,
        );
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    }

    /// A stub `gh` first on `PATH` and a bare `origin` to push to; returns
    /// the log of `gh` calls.
    pub fn stub_gh(&self) -> PathBuf {
        let bin = self.faults.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, GH).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let origin = self.faults.join("origin.git");
        git(
            &self.repo,
            &["init", "--quiet", "--bare", origin.to_str().unwrap()],
        );
        git(
            &self.repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        bin.join("gh.log")
    }

    /// `aver accept args` in `dir`, asserting it succeeds.
    pub fn accept(&self, dir: &Path, args: &[&str]) -> String {
        let mut all = vec!["accept"];
        all.extend_from_slice(args);
        let out = self.aver(dir, &all, None);
        assert!(out.status.success(), "aver {all:?}: {}", text(&out));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Waits until a process paused at `point` says so; returns its pid.
    pub fn paused(&self, point: &str) -> u32 {
        let file = self.faults.join(format!("{point}.paused"));
        for _ in 0..1500 {
            if let Ok(pid) = fs::read_to_string(&file) {
                return pid.trim().parse().unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("nothing paused at {point}");
    }

    pub fn resume(&self, point: &str) {
        fs::write(self.faults.join(format!("{point}.go")), "").unwrap();
    }
}
