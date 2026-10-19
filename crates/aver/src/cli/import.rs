use aver::agents::AgentName;
use aver::git::{self, ImportOptions, RefPrecondition, RefUpdate, SecretPolicy};
use aver::keys::Home;
use aver::services::{key_in, Services};
use aver_core::RefName;
use serde_json::{json, Value};

#[derive(clap::Args)]
pub struct Args {
    /// A local repository path or a URL.
    repo: String,
    /// The revision to import.
    #[arg(long, default_value = "HEAD")]
    rev: String,
    /// A ref to point at the imported snapshot.
    #[arg(long = "ref")]
    reference: Option<RefName>,
    /// The key the import is recorded under (default: admin).
    #[arg(long)]
    key: Option<AgentName>,
    /// What a possible secret does: reject the import (the default) or warn and import anyway.
    #[arg(long, value_enum, default_value_t = SecretsArg::Reject)]
    secrets: SecretsArg,
}

/// `--secrets` for imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SecretsArg {
    Reject,
    Warn,
}

impl From<SecretsArg> for SecretPolicy {
    fn from(arg: SecretsArg) -> Self {
        match arg {
            SecretsArg::Reject => Self::Reject,
            SecretsArg::Warn => Self::Warn,
        }
    }
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let key = key_in(home, args.key.as_ref())?;
    let out = import(
        &Services::open(home)?,
        &key,
        &args.repo,
        &args.rev,
        args.reference,
        args.secrets.into(),
    )?;
    for finding in out["secrets"].as_array().into_iter().flatten() {
        eprintln!(
            "aver: warning: {} looks like it contains a secret ({}) ({} severity); imported anyway",
            finding["path"].as_str().unwrap_or("?"),
            finding["pattern"].as_str().unwrap_or("?"),
            finding["severity"].as_str().unwrap_or("?"),
        );
    }
    let out = json!({
        "snapshot": out["snapshot"],
        "env": out["env"],
        "transition": out["transition"],
        "commit": out["commit"],
        "files": out["files"],
        "already_imported": out["already_imported"],
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Imports `rev` of `repo` into `services`, returning the import report
/// as JSON (`snapshot`, `env`, `transition`, `commit`, `files`,
/// `already_imported`, and `secrets` when `secrets` is
/// [`SecretPolicy::Warn`] and the scanner found any). A local path is made absolute.
pub(super) fn import(
    services: &Services,
    key: &aver::keys::AgentKey,
    repo: &str,
    rev: &str,
    reference: Option<RefName>,
    secrets: SecretPolicy,
) -> anyhow::Result<Value> {
    let repo = match std::fs::canonicalize(repo) {
        Ok(path) if !repo.contains("://") => path.to_string_lossy().into_owned(),
        _ => repo.to_owned(),
    };
    let mut options = ImportOptions::new(
        &repo,
        rev,
        key.id(),
        &services.server_key,
        &*services.images,
    );
    options.secrets = secrets;
    options.update_ref = reference.map(|name| RefUpdate {
        name,
        expect: RefPrecondition::Any,
    });
    let report = git::import(&services.store, &options)?;
    let mut out = json!({
        "snapshot": report.snapshot,
        "env": report.env,
        "transition": report.transition,
        "commit": report.commit,
        "files": report.files,
        "already_imported": report.already_imported,
    });
    if !report.secrets.is_empty() {
        out["secrets"] = json!(report.secrets);
    }
    Ok(out)
}
