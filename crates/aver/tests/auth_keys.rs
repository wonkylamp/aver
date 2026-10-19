//! Key files, `aver init` and `aver agent add` (§6.3).

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use aver::agents::{self, AgentKind, Capability};
use aver::keys::{self, AgentKey, Home, KeyError};
use aver_core::{AgentId, SigningKey};
use data_encoding::BASE64URL_NOPAD;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn aver(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(args)
        .env("AVER_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn init_writes_private_key_files_in_the_documented_format() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::new(dir.path().join("home"));
    let init = keys::init(&home).unwrap();
    assert!(init.changed);
    assert_eq!(mode(&home.keys_dir()), 0o700);

    let path = home.keys_dir().join("admin.json");
    assert_eq!(mode(&path), 0o600);
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let fields: BTreeSet<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        BTreeSet::from(["agent_id", "name", "public_key", "secret_key"])
    );
    let seed: [u8; 32] = BASE64URL_NOPAD
        .decode(json["secret_key"].as_str().unwrap().as_bytes())
        .unwrap()
        .try_into()
        .unwrap();
    let key = SigningKey::from_bytes(&seed);
    assert_eq!(
        json["public_key"],
        BASE64URL_NOPAD.encode(key.verifying_key().as_bytes())
    );
    assert_eq!(
        json["agent_id"],
        AgentId::of_public_key(&key.verifying_key()).to_string()
    );
    assert_eq!(json["name"], "admin");
    assert_eq!(mode(&home.keys_dir().join("server.json")), 0o600);

    let store = home.open_store().unwrap();
    let admin = agents::get(&store, &init.admin.id()).unwrap().unwrap();
    assert_eq!(admin.capabilities, BTreeSet::from([Capability::Admin]));
    assert_eq!(admin.issued_by, None);
    let server = agents::get(&store, &init.server.id()).unwrap().unwrap();
    assert_eq!(server.kind, AgentKind::Server);
    assert!(server.capabilities.is_empty());
}

#[test]
fn init_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::new(dir.path());
    let first = keys::init(&home).unwrap();
    let second = keys::init(&home).unwrap();
    assert!(!second.changed);
    assert_eq!(first.admin.id(), second.admin.id());
    assert_eq!(first.server.id(), second.server.id());

    std::fs::remove_file(home.db_path()).unwrap();
    let third = keys::init(&home).unwrap();
    assert!(
        third.changed,
        "existing keys are re-registered in a fresh database"
    );
    assert_eq!(third.admin.id(), first.admin.id());
}

#[test]
fn key_files_whose_fields_disagree_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys/k.json");
    let key = AgentKey::generate("k".parse().unwrap());
    key.write(&path).unwrap();
    assert_eq!(AgentKey::read(&path).unwrap().id(), key.id());
    assert!(matches!(key.write(&path), Err(KeyError::Exists(_))));

    let mut file = key.to_file();
    file.public_key =
        BASE64URL_NOPAD.encode(SigningKey::from_bytes(&[1; 32]).verifying_key().as_bytes());
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    assert!(matches!(
        AgentKey::read(&path),
        Err(KeyError::Malformed { .. })
    ));
}

#[test]
fn cli_init_and_agent_add() {
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let first = aver(&home_dir, &["init"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = aver(&home_dir, &["init"]);
    assert!(second.status.success());
    let ids = |out: &std::process::Output| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&first), ids(&second));
    assert!(String::from_utf8_lossy(&second.stdout).starts_with("already initialized"));

    let add = aver(
        &home_dir,
        &[
            "agent",
            "add",
            "--name",
            "worker-1",
            "--caps",
            "workspace:write,exec:run",
        ],
    );
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let home = Home::new(&home_dir);
    let key_path = home.keys_dir().join("worker-1.json");
    assert_eq!(mode(&key_path), 0o600);
    let key = AgentKey::read(&key_path).unwrap();
    assert!(String::from_utf8_lossy(&add.stdout).starts_with(&key.id().to_string()));

    let store = home.open_store().unwrap();
    let agent = agents::get(&store, &key.id()).unwrap().unwrap();
    assert_eq!(
        agent.capabilities,
        BTreeSet::from([Capability::WorkspaceWrite, Capability::ExecRun])
    );
    let admin = AgentKey::read(&home.keys_dir().join("admin.json")).unwrap();
    assert_eq!(agent.issued_by, Some(admin.id()));

    let again = aver(
        &home_dir,
        &["agent", "add", "--name", "worker-1", "--caps", "exec:run"],
    );
    assert!(!again.status.success());
    let bad = aver(
        &home_dir,
        &["agent", "add", "--name", "w2", "--caps", "root"],
    );
    assert!(!bad.status.success());
    assert!(!home.keys_dir().join("w2.json").exists());
}

/// `aver key` prints only the verifier public key, creating the home on
/// first use and printing the same key afterwards; `--home` overrides
/// `AVER_HOME`.
#[test]
fn key_prints_the_verifier_public_key_and_initializes_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let unused = dir.path().join("unused");
    let home_dir = dir.path().join("fresh");
    let home_arg = home_dir.to_str().unwrap();
    let first = aver(&unused, &["key", "--home", home_arg]);
    assert!(first.status.success(), "{first:?}");
    assert!(String::from_utf8_lossy(&first.stderr).contains("initialized"));
    let printed = String::from_utf8(first.stdout).unwrap();
    let server = AgentKey::read(&Home::new(&home_dir).keys_dir().join("server.json")).unwrap();
    assert_eq!(printed, format!("{}\n", server.to_file().public_key));
    assert_eq!(printed.trim().len(), 43);
    assert!(!unused.exists());

    let again = aver(&unused, &["key", "--home", home_arg]);
    assert_eq!(String::from_utf8(again.stdout).unwrap(), printed);
    assert!(again.stderr.is_empty());
    let from_env = aver(&home_dir, &["key"]);
    assert_eq!(String::from_utf8(from_env.stdout).unwrap(), printed);
}
