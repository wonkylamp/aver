//! The GitHub Actions workflow `aver init --ci` writes: the `aver-verify`
//! action on pull requests (`review: approve`), pushes to the default branch
//! (which record the base's runs for later pull requests), merge queues
//! (regression-only) and reviews (`mode: approve`), after the steps that
//! install the repository's toolchains and dependencies, so the checks find
//! them in the job's checkout.
//!
//! Only actions whose commit is known here are used: `actions/checkout`,
//! `actions/setup-node` and Aver's own. Python, Rust and Go are set up with
//! `run:` steps (uv, rustup and the runner image's Go) instead of pinning an
//! action to a commit nobody checked.

use std::path::Path;

use crate::env::Family;
use crate::presets::{MAX_DEPTH, SKIP};

/// Where the workflow goes, relative to the repository root.
pub const PATH: &str = ".github/workflows/aver.yml";

/// The action's pin: the commit the release workflow built this binary
/// from (it sets `AVER_ACTION_SHA`), else the newest pushed commit whose
/// action has every input the workflow uses.
pub const ACTION_SHA: &str = match option_env!("AVER_ACTION_SHA") {
    Some(sha) => sha,
    None => "c7adef53d49afd28fe2e8ded8733a70dd607369a",
};
const ACTION: &str = "wonkylamp/aver/action";
const CHECKOUT: &str = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1";
const SETUP_NODE: &str = "actions/setup-node@949feb2413d6458794dcd2491c4babbbce0c15c1 # v7.1.0";

/// The check run branch protection requires.
pub const CHECK_NAME: &str = "Aver";

/// What the workflow is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The branch whose pushes record base runs.
    pub branch: String,
    /// The verify job's steps between the checkout and the action.
    pub setup: Vec<Step>,
}

/// One step of the verify job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub body: Body,
    /// Relative to the repository root; `None` at the root.
    pub dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// A pinned action with its `with:` inputs.
    Uses(&'static str, Vec<(&'static str, String)>),
    Run(String),
}

/// The project files found in one directory.
struct Dir {
    /// Relative to the root, `""` for the root.
    path: String,
    files: Vec<String>,
}

impl Dir {
    fn has(&self, name: &str) -> bool {
        self.files.iter().any(|f| f == name)
    }

    fn step(&self, name: &str, run: String) -> Step {
        let at = if self.path.is_empty() {
            String::new()
        } else {
            format!(" in {}", self.path)
        };
        Step {
            name: format!("{name}{at}"),
            body: Body::Run(run),
            dir: (!self.path.is_empty()).then(|| self.path.clone()),
        }
    }
}

/// The plan for the working tree at `root`, whose pushes to `branch` record
/// base runs. Directories are searched as generic mode searches them; one
/// whose path YAML or a shell would need quoted is left out.
pub fn plan(root: &Path, branch: &str) -> Plan {
    let mut dirs = Vec::new();
    walk(root, "", 0, &mut dirs);
    let root_dir = dirs.iter().find(|d| d.path.is_empty());
    let root_has = |name: &str| root_dir.is_some_and(|d| d.has(name));
    let mut setup = Vec::new();
    for family in [Family::Node, Family::Python, Family::Rust, Family::Go] {
        let installs: Vec<Step> = dirs.iter().filter_map(|d| install(family, d)).collect();
        if installs.is_empty() {
            continue;
        }
        setup.extend(toolchain(family, &installs, &root_has));
        setup.extend(installs);
    }
    Plan {
        branch: branch.to_owned(),
        setup,
    }
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<Dir>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < MAX_DEPTH && !name.starts_with('.') && !SKIP.contains(&name.as_str()) {
                subdirs.push(name);
            }
        } else {
            files.push(name);
        }
    }
    files.sort();
    subdirs.sort();
    out.push(Dir {
        path: rel.to_owned(),
        files,
    });
    for name in subdirs {
        if !plain(&name) {
            continue;
        }
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        walk(&dir.join(&name), &child, depth + 1, out);
    }
}

/// A name that needs no quoting in YAML or a shell.
fn plain(name: &str) -> bool {
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The step installing `dir`'s dependencies of `family`, from its lockfile.
fn install(family: Family, dir: &Dir) -> Option<Step> {
    match family {
        // A nested package counts only with a lockfile, as generic mode
        // requires (`languages`).
        Family::Node => {
            let run = if dir.has("package-lock.json") || dir.has("npm-shrinkwrap.json") {
                "npm ci"
            } else if dir.has("pnpm-lock.yaml") {
                "pnpm install --frozen-lockfile"
            } else if dir.has("yarn.lock") {
                "yarn install --frozen-lockfile"
            } else if dir.path.is_empty() && dir.has("package.json") {
                "npm install"
            } else {
                return None;
            };
            Some(dir.step("Install the Node dependencies", run.into()))
        }
        Family::Python => {
            let reqs: Vec<&String> = dir
                .files
                .iter()
                .filter(|f| f.starts_with("requirements") && f.ends_with(".txt"))
                .collect();
            let install = if dir.has("uv.lock") {
                "uv sync --frozen".to_owned()
            } else if !reqs.is_empty() {
                let files: Vec<String> = reqs
                    .iter()
                    .filter(|f| plain(f))
                    .map(|f| format!("-r {f}"))
                    .collect();
                format!("uv venv\nuv pip install {}", files.join(" "))
            } else if ["pyproject.toml", "setup.py", "setup.cfg"]
                .iter()
                .any(|f| dir.has(f))
            {
                "uv venv".to_owned()
            } else {
                return None;
            };
            // Checks inherit the job's PATH, so they run this
            // virtualenv's python and pytest.
            let run = format!("{install}\necho \"$PWD/.venv/bin\" >>\"$GITHUB_PATH\"");
            Some(dir.step("Install the Python dependencies", run))
        }
        Family::Rust => dir
            .has("Cargo.lock")
            .then(|| dir.step("Fetch the Rust dependencies", "cargo fetch --locked".into())),
        Family::Go => (dir.has("go.mod") && dir.has("go.sum"))
            .then(|| dir.step("Download the Go modules", "go mod download".into())),
    }
}

/// The steps that put `family`'s toolchain on the runner, before
/// `installs`.
fn toolchain(family: Family, installs: &[Step], root_has: &dyn Fn(&str) -> bool) -> Vec<Step> {
    let run = |name: &str, run: &str| Step {
        name: name.into(),
        body: Body::Run(run.into()),
        dir: None,
    };
    match family {
        Family::Node => {
            let version = [".nvmrc", ".node-version"]
                .into_iter()
                .find(|f| root_has(f))
                .map_or(("node-version", "lts/*".to_owned()), |f| {
                    ("node-version-file", f.to_owned())
                });
            let mut steps = vec![Step {
                name: "Install Node".into(),
                body: Body::Uses(SETUP_NODE, vec![version]),
                dir: None,
            }];
            let corepack = installs.iter().any(|s| {
                matches!(&s.body, Body::Run(r) if r.starts_with("pnpm") || r.starts_with("yarn"))
            });
            if corepack {
                steps.push(run("Enable pnpm and Yarn", "corepack enable"));
            }
            steps
        }
        // uv reads `.python-version` and `requires-python`, and installs
        // that Python when the runner lacks it.
        Family::Python => vec![run("Install uv", "pipx install uv")],
        Family::Rust => {
            let pinned = root_has("rust-toolchain.toml") || root_has("rust-toolchain");
            vec![if pinned {
                run("Install Rust", "rustup toolchain install")
            } else {
                run(
                    "Install Rust",
                    "rustup toolchain install stable --profile minimal --no-self-update\nrustup default stable",
                )
            }]
        }
        // The runner image's Go; a go.mod asking for a newer one fetches it
        // (GOTOOLCHAIN=auto) during the download.
        Family::Go => Vec::new(),
    }
}

/// The workflow's text.
pub fn render(plan: &Plan) -> String {
    let mut setup = String::new();
    for step in &plan.setup {
        setup.push_str(&format!("      - name: {}\n", step.name));
        match &step.body {
            Body::Uses(action, with) => {
                setup.push_str(&format!("        uses: {action}\n        with:\n"));
                for (k, v) in with {
                    setup.push_str(&format!("          {k}: '{}'\n", v.replace('\'', "''")));
                }
            }
            Body::Run(run) => {
                if let Some(dir) = &step.dir {
                    setup.push_str(&format!("        working-directory: {dir}\n"));
                }
                if run.contains('\n') {
                    setup.push_str("        run: |\n");
                    for line in run.lines() {
                        setup.push_str(&format!("          {line}\n"));
                    }
                } else {
                    setup.push_str(&format!("        run: {run}\n"));
                }
            }
        }
    }
    TEMPLATE
        .replace("{branch}", &plan.branch)
        .replace("{checkout}", CHECKOUT)
        .replace("{action}", &action_ref())
        .replace("{setup}", &setup)
}

/// `owner/repo/action@<sha> # v<version>`.
pub fn action_ref() -> String {
    format!("{ACTION}@{ACTION_SHA} # v{}", env!("CARGO_PKG_VERSION"))
}

const TEMPLATE: &str = r#"# Aver verifies every pull request against its base and posts a check run
# named Aver. Written by `aver init --ci`; docs/CI.md explains it.
#
# verify, on a pull request: runs the checks on the base and the head and
# posts Aver as success, failure, or action_required when a maintainer has
# to look (REVIEW or UNPROVEN). On a push to {branch} it records its runs,
# so pull requests based on that commit do not run the base again. In a
# merge queue it fails only on a regression.
# approve, on a review: when a person other than the author approved the
# pull request's head commit, posts Aver as success on it.
name: Aver

on:
  pull_request:
  pull_request_review:
    types: [submitted, dismissed]
  push:
    branches: [{branch}]
  merge_group:

permissions:
  contents: read

concurrency:
  group: aver-${{ github.event_name }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  verify:
    if: github.event_name != 'pull_request_review'
    runs-on: ubuntu-latest
    permissions:
      contents: read
      checks: write
      pull-requests: write
    steps:
      # The pull request's head, not GitHub's merge commit, so the checks
      # run in this checkout with the dependencies installed below. The
      # full history means no fetch, so no token stays in .git/config.
      - uses: {checkout}
        with:
          ref: ${{ github.event.pull_request.head.sha || github.sha }}
          fetch-depth: 0
          persist-credentials: false
{setup}      - uses: {action}
        with:
          review: approve

  approve:
    if: github.event_name == 'pull_request_review'
    runs-on: ubuntu-latest
    permissions:
      contents: read
      checks: write
      pull-requests: read
    steps:
      - uses: {checkout}
        with:
          ref: ${{ github.event.pull_request.head.sha }}
          fetch-depth: 0
          persist-credentials: false
      - uses: {action}
        with:
          mode: approve
"#;

/// What to tell the maintainer after the workflow: each job, then how to
/// require the check. At most 15 lines.
pub fn explain(plan: &Plan) -> String {
    let branch = &plan.branch;
    format!(
        "\
The workflow runs the action on four events:
  pull_request         verify the head against the base; post the {CHECK_NAME} check run
  pull_request_review  a maintainer's approval of the head commit clears REVIEW or UNPROVEN
  push to {branch:<12} record the runs, so pull requests on it skip their base
  merge_group          fail only on a regression of the queued merge
Commit it, then require the check named {CHECK_NAME} on {branch} (with the action's `check`
input in a matrix, require each `{CHECK_NAME} (<check>)`). For a merge queue, turn it on
in the same rule. This command sets the rule; run it yourself:
gh api -X PUT 'repos/{{owner}}/{{repo}}/branches/{branch}/protection' --input - <<'EOF'
{{\"required_status_checks\":{{\"strict\":false,\"checks\":[{{\"context\":\"{CHECK_NAME}\"}}]}},\"enforce_admins\":false,\"required_pull_request_reviews\":null,\"restrictions\":null}}
EOF
"
    )
}
