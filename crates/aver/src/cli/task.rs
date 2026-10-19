use std::path::PathBuf;

use anyhow::Context;
use aver::agents::AgentName;
use aver::keys::Home;
use aver::services::{key_in, Services};
use aver_core::{
    Criteria, Object, PathGlob, RefName, Set, SnapshotId, Task, TaskBody, TaskCheckDef, TaskId,
    TaskSchema, Text, Timestamp,
};
use serde::Deserialize;
use serde_json::json;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Sign a task and store it.
    Create {
        /// JSON with title, intent, base, criteria and optionally target,
        /// checks and protected_paths; the author and creation time are added
        /// when signing.
        #[arg(long)]
        file: PathBuf,
        /// The author's key (default: admin).
        #[arg(long)]
        key: Option<AgentName>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    title: Text<{ aver_core::task::MAX_TITLE_BYTES }>,
    intent: Text<{ aver_core::task::MAX_INTENT_BYTES }>,
    base: SnapshotId,
    #[serde(default)]
    target: Option<RefName>,
    criteria: Criteria,
    #[serde(default)]
    checks: Vec<TaskCheckDef>,
    #[serde(default)]
    protected_paths: std::collections::BTreeSet<PathGlob>,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    match args.command {
        Command::Create { file, key } => {
            let author = key_in(home, key.as_ref())?;
            let text =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let draft: Draft = serde_json::from_slice(&text)
                .with_context(|| format!("parsing {}", file.display()))?;
            let protected_paths = Set::new(draft.protected_paths.into_iter().collect())?;
            let body = TaskBody {
                schema: TaskSchema::for_task(&draft.criteria, &protected_paths),
                title: draft.title,
                intent: draft.intent,
                base: draft.base,
                target: draft.target,
                criteria: draft.criteria,
                checks: Set::new(draft.checks)?,
                protected_paths,
                author: author.id(),
                created_at: Timestamp::now(),
            };
            let task = Task::sign(body, &author.signing_key);
            create(&Services::open(home)?, &task)?;
            let out = json!({ "id": task.id(), "task": task });
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(())
        }
    }
}

/// Stores a signed task.
pub(super) fn create(services: &Services, task: &Task) -> anyhow::Result<TaskId> {
    services.tasks.create(task)?;
    Ok(task.id())
}
