//! `$AVER_HOME` layout and agent key files (ARCHITECTURE.md §3, §6.3).
//!
//! Keys live in `$AVER_HOME/keys` (directory mode 0700, files 0600), one
//! JSON file per key: `{"agent_id","name","public_key","secret_key"}`, where
//! `public_key` is the raw 32-byte Ed25519 public key and `secret_key` the
//! 32-byte seed, both unpadded base64url. Reading a key file checks that the
//! three key fields agree. [`init`] creates the server key and the bootstrap
//! admin and registers both; running it again changes nothing.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use aver_core::{AgentId, SigningKey};
use data_encoding::BASE64URL_NOPAD;
use serde::{Deserialize, Serialize};

use crate::agents::{self, AgentError, AgentKind, AgentName, Capability, NewAgent};
use crate::store::{Store, StoreError};

pub const SERVER_KEY: &str = "server";
pub const ADMIN_KEY: &str = "admin";

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: malformed key file: {reason}")]
    Malformed { path: PathBuf, reason: String },
    #[error("{0} already exists")]
    Exists(PathBuf),
    #[error("{0} not found; run `aver init` first")]
    Missing(PathBuf),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The data directory: `aver.db`, `keys/`, `tmp/`.
#[derive(Debug, Clone)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$AVER_HOME`, else `~/.aver`.
    pub fn from_env() -> Self {
        if let Some(dir) = std::env::var_os("AVER_HOME").filter(|v| !v.is_empty()) {
            return Self::new(dir);
        }
        let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
        Self::new(PathBuf::from(home).join(".aver"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("aver.db")
    }

    pub fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }

    pub fn key_path(&self, name: &AgentName) -> PathBuf {
        self.keys_dir().join(format!("{name}.json"))
    }

    pub fn open_store(&self) -> Result<Store, StoreError> {
        Store::open(self.db_path())
    }

    /// Takes the home's exclusive advisory lock (`<home>/lock`, created with
    /// the home), calling `on_wait` first if another process holds it, and
    /// holds it until the guard drops. Commands run side by side on one home
    /// (§2.7.3), so only what must not overlap anything takes it: creating
    /// the keys ([`init`]), `aver gc`, and `aver ci` on its own home.
    pub fn lock(&self, on_wait: impl FnOnce()) -> Result<HomeLock, KeyError> {
        let (file, got) = self.lock_file()?;
        if !got {
            on_wait();
            let path = self.root.join("lock");
            file.lock()
                .map_err(|source| KeyError::Io { path, source })?;
        }
        Ok(HomeLock { _file: file })
    }

    /// The home's lock if nobody holds it, without waiting.
    pub fn try_lock(&self) -> Result<Option<HomeLock>, KeyError> {
        let (file, got) = self.lock_file()?;
        Ok(got.then_some(HomeLock { _file: file }))
    }

    fn lock_file(&self) -> Result<(std::fs::File, bool), KeyError> {
        let path = self.root.join("lock");
        let io = |source| KeyError::Io {
            path: path.clone(),
            source,
        };
        std::fs::create_dir_all(&self.root).map_err(io)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io)?;
        match file.try_lock() {
            Ok(()) => Ok((file, true)),
            Err(std::fs::TryLockError::WouldBlock) => Ok((file, false)),
            Err(std::fs::TryLockError::Error(e)) => Err(io(e)),
        }
    }
}

/// Holds a home's lock ([`Home::lock`]); the lock is released when the file
/// closes, also when the process dies.
#[derive(Debug)]
pub struct HomeLock {
    _file: std::fs::File,
}

/// An agent's name and private key.
pub struct AgentKey {
    pub name: AgentName,
    pub signing_key: SigningKey,
}

impl AgentKey {
    pub fn generate(name: AgentName) -> Self {
        Self {
            name,
            signing_key: SigningKey::from_bytes(&rand::random::<[u8; 32]>()),
        }
    }

    pub fn id(&self) -> AgentId {
        AgentId::of_public_key(&self.signing_key.verifying_key())
    }

    pub fn to_file(&self) -> KeyFile {
        KeyFile {
            agent_id: self.id(),
            name: self.name.to_string(),
            public_key: BASE64URL_NOPAD.encode(self.signing_key.verifying_key().as_bytes()),
            secret_key: BASE64URL_NOPAD.encode(self.signing_key.as_bytes()),
        }
    }

    /// Writes the key to `path` with mode 0600, refusing to overwrite.
    pub fn write(&self, path: &Path) -> Result<(), KeyError> {
        let io = |source| KeyError::Io {
            path: path.to_owned(),
            source,
        };
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        let mut json = serde_json::to_vec_pretty(&self.to_file()).expect("key files serialize");
        json.push(b'\n');
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                KeyError::Exists(path.to_owned())
            } else {
                io(e)
            }
        })?;
        file.write_all(&json).map_err(io)?;
        file.sync_all().map_err(io)
    }

    pub fn read(path: &Path) -> Result<Self, KeyError> {
        let bytes = std::fs::read(path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                KeyError::Missing(path.to_owned())
            } else {
                KeyError::Io {
                    path: path.to_owned(),
                    source,
                }
            }
        })?;
        let malformed = |reason: String| KeyError::Malformed {
            path: path.to_owned(),
            reason,
        };
        let file: KeyFile = serde_json::from_slice(&bytes).map_err(|e| malformed(e.to_string()))?;
        let seed: [u8; 32] = aver_core::decode_base64url(&file.secret_key)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| malformed("secret_key is not 32 bytes of base64url".into()))?;
        let key = Self {
            name: file
                .name
                .parse()
                .map_err(|e: AgentError| malformed(e.to_string()))?,
            signing_key: SigningKey::from_bytes(&seed),
        };
        let expected = key.to_file();
        if expected.public_key != file.public_key {
            return Err(malformed("public_key does not match secret_key".into()));
        }
        if expected.agent_id != file.agent_id {
            return Err(malformed("agent_id does not match public_key".into()));
        }
        Ok(key)
    }
}

/// The on-disk key file format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyFile {
    pub agent_id: AgentId,
    pub name: String,
    pub public_key: String,
    pub secret_key: String,
}

fn create_private_dir(dir: &Path) -> Result<(), KeyError> {
    let io = |source| KeyError::Io {
        path: dir.to_owned(),
        source,
    };
    std::fs::create_dir_all(dir).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
    }
    Ok(())
}

pub struct Initialized {
    pub server: AgentKey,
    pub admin: AgentKey,
    /// Whether this call created or registered anything.
    pub changed: bool,
}

/// Creates the server key and the bootstrap admin key if missing, and
/// registers any that are not yet in the agent registry.
pub fn init(home: &Home) -> Result<Initialized, KeyError> {
    let lock = home.lock(|| {})?;
    init_locked(home, &lock)
}

/// [`init`] for a caller that already holds the home's lock (a second
/// `flock` from the same process would wait on the first).
pub fn init_locked(home: &Home, _held: &HomeLock) -> Result<Initialized, KeyError> {
    create_private_dir(&home.keys_dir())?;
    std::fs::create_dir_all(home.root().join("tmp")).map_err(|source| KeyError::Io {
        path: home.root().join("tmp"),
        source,
    })?;
    let store = home.open_store()?;
    let mut changed = false;
    let mut load_or_create = |name: &str| -> Result<AgentKey, KeyError> {
        let name: AgentName = name.parse()?;
        let path = home.key_path(&name);
        match AgentKey::read(&path) {
            Ok(key) => Ok(key),
            Err(KeyError::Missing(_)) => {
                let key = AgentKey::generate(name);
                key.write(&path)?;
                changed = true;
                Ok(key)
            }
            Err(e) => Err(e),
        }
    };
    let admin = load_or_create(ADMIN_KEY)?;
    let server = load_or_create(SERVER_KEY)?;
    if agents::get(&store, &admin.id())?.is_none() {
        agents::register(
            &store,
            NewAgent {
                public_key: admin.signing_key.verifying_key(),
                name: admin.name.clone(),
                kind: AgentKind::Agent,
                capabilities: BTreeSet::from([Capability::Admin]),
                issued_by: None,
            },
        )?;
        changed = true;
    }
    if agents::get(&store, &server.id())?.is_none() {
        agents::register(
            &store,
            NewAgent {
                public_key: server.signing_key.verifying_key(),
                name: server.name.clone(),
                kind: AgentKind::Server,
                capabilities: BTreeSet::new(),
                issued_by: Some(admin.id()),
            },
        )?;
        changed = true;
    }
    Ok(Initialized {
        server,
        admin,
        changed,
    })
}

/// Creates a key file for a new agent named `name` and registers it, issued
/// by the bootstrap admin.
pub fn add_agent(
    home: &Home,
    store: &Store,
    name: AgentName,
    capabilities: BTreeSet<Capability>,
) -> Result<(AgentKey, PathBuf), KeyError> {
    let admin = AgentKey::read(&home.key_path(&ADMIN_KEY.parse()?))?;
    let path = home.key_path(&name);
    if path.exists() {
        return Err(KeyError::Exists(path));
    }
    let key = AgentKey::generate(name);
    key.write(&path)?;
    let registered = agents::register(
        store,
        NewAgent {
            public_key: key.signing_key.verifying_key(),
            name: key.name.clone(),
            kind: AgentKind::Agent,
            capabilities,
            issued_by: Some(admin.id()),
        },
    );
    if let Err(e) = registered {
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    Ok((key, path))
}
