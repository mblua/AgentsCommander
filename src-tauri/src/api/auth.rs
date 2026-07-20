//! Scoped, revocable, per-client API token registry (#791 §5).
//!
//! The store is host-only (`api-clients.json` in `config_dir()`, outside any
//! workgroup mount). Tokens are stored HASHED (SHA-256); the plaintext is shown
//! once at mint and never persisted. Because `api-client mint`/`revoke` run in a
//! SEPARATE CLI process, the daemon's auth check reads the file THROUGH to disk
//! on every request via an mtime-gated reload cache (§0.5 HIGH-2): a CLI-side
//! mint/revoke takes effect on the next request. The file is the source of
//! truth; the in-memory copy is a cache keyed by mtime.

use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::error::ApiError;

/// The `send` scope (POST /api/v1/send).
pub const SCOPE_SEND: &str = "send";
/// The `list-peers-lean` scope (GET /api/v1/peers).
pub const SCOPE_LIST_PEERS: &str = "list-peers-lean";
/// The container session transport scope (GET /api/v1/session-transport).
pub const SCOPE_SESSION_TRANSPORT: &str = "session-transport";
/// Dedicated privileged exact PTY-input scope.
pub const SCOPE_PTY_INPUT: &str = "pty-input";
/// Scopes accepted by the registry. Possession of `pty-input` alone is not
/// authority; the handler also requires an automatic live container binding.
pub const VALID_SCOPES: &[&str] = &[
    SCOPE_SEND,
    SCOPE_LIST_PEERS,
    SCOPE_SESSION_TRANSPORT,
    SCOPE_PTY_INPUT,
];

/// Registry file basename in `config_dir()`.
pub const REGISTRY_FILENAME: &str = "api-clients.json";

/// One registered API client. `boundRoot` is the identity source (the `from` is
/// derived from it at request time); `boundFqn` is an audit hint only (§0.5 G5).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiClient {
    /// Stable id, safe to log.
    pub client_id: String,
    /// Human note.
    #[serde(default)]
    pub label: String,
    /// `"sha256:<hex>"` of the secret. The plaintext is never stored.
    pub token_hash: String,
    /// Audit/log hint of the bound identity at mint time (may go stale).
    pub bound_fqn: String,
    /// The replica working directory: the authoritative identity source.
    pub bound_root: String,
    /// Allowlisted ops (verb names).
    pub scopes: Vec<String>,
    /// RFC3339 mint time.
    pub issued_at: String,
    /// Optional RFC3339 expiry, or `null`.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// Revocation flag.
    #[serde(default)]
    pub revoked: bool,
    /// Present only on automatically minted container credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_session_id: Option<String>,
    /// Fresh automatic credential generation, paired with `boundSessionId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_generation: Option<String>,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApiClient")
            .field("client_id", &self.client_id)
            .field("label", &self.label)
            .field("token_hash", &"[REDACTED]")
            .field("bound_fqn", &self.bound_fqn)
            .field("bound_root", &"[REDACTED_PATH]")
            .field("scopes", &self.scopes)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("revoked", &self.revoked)
            .field("bound_session_id", &self.bound_session_id)
            .field("credential_generation", &self.credential_generation)
            .finish()
    }
}

impl ApiClient {
    /// Whether this client holds the given scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// On-disk registry document.
#[derive(Clone, Serialize, Deserialize)]
pub struct ApiClientRegistry {
    pub version: u32,
    #[serde(default)]
    pub clients: Vec<ApiClient>,
}

impl std::fmt::Debug for ApiClientRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApiClientRegistry")
            .field("version", &self.version)
            .field("clients", &self.clients)
            .finish()
    }
}

impl Default for ApiClientRegistry {
    fn default() -> Self {
        Self {
            version: 1,
            clients: Vec::new(),
        }
    }
}

/// Diagnostic state from loading the host API client registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryLoadProblem {
    pub status: &'static str,
    pub message: String,
}

/// Registry plus any fail-closed load problem operators should see.
#[derive(Clone)]
pub struct RegistrySnapshot {
    pub registry: ApiClientRegistry,
    pub problem: Option<RegistryLoadProblem>,
}

impl std::fmt::Debug for RegistrySnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegistrySnapshot")
            .field("registry", &self.registry)
            .field("problem", &self.problem)
            .finish()
    }
}

/// SHA-256 the secret, formatted `"sha256:<lowercase-hex>"`.
pub fn hash_token(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        // Two lowercase hex chars per byte.
        hex.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        hex.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    format!("sha256:{}", hex)
}

/// Constant-time equality over two equal-length strings (the fixed-length hex
/// digests). Mirrors `MasterToken::matches` (`lib.rs:89`). The length check
/// leaks only length, which for fixed-length digests is a constant.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Whether the client is expired as of `now`. A present-but-unparseable
/// `expiresAt` is treated as EXPIRED (fail-closed).
pub fn is_expired(client: &ApiClient, now: chrono::DateTime<chrono::Utc>) -> bool {
    match client.expires_at.as_deref() {
        None => false,
        Some(raw) => match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(exp) => exp.with_timezone(&chrono::Utc) <= now,
            Err(_) => true,
        },
    }
}

/// Load the registry, tolerating an absent/malformed/unreadable file by
/// returning an empty registry (fail-safe: no clients => everything 401).
fn load_registry(path: &Path) -> RegistrySnapshot {
    match std::fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(registry) => RegistrySnapshot {
                registry,
                problem: None,
            },
            Err(e) => {
                let message = format!("{} is malformed: {}", REGISTRY_FILENAME, e);
                log::error!(
                    "[api-auth] {} is malformed ({}); treating as empty registry",
                    REGISTRY_FILENAME,
                    e
                );
                RegistrySnapshot {
                    registry: ApiClientRegistry::default(),
                    problem: Some(RegistryLoadProblem {
                        status: "malformed",
                        message,
                    }),
                }
            }
        },
        Err(e) if e.kind() == ErrorKind::NotFound => RegistrySnapshot {
            registry: ApiClientRegistry::default(),
            problem: None,
        },
        Err(e) => {
            let message = format!("{} is unreadable: {}", REGISTRY_FILENAME, e);
            log::error!(
                "[api-auth] {} is unreadable ({}); treating as empty registry",
                REGISTRY_FILENAME,
                e
            );
            RegistrySnapshot {
                registry: ApiClientRegistry::default(),
                problem: Some(RegistryLoadProblem {
                    status: "unreadable",
                    message,
                }),
            }
        }
    }
}

struct CacheInner {
    mtime: Option<SystemTime>,
    loaded: bool,
    registry: ApiClientRegistry,
    problem: Option<RegistryLoadProblem>,
}

/// Read-through, mtime-gated registry cache. The source of truth is the file on
/// disk (written by the separate CLI process); the cache reloads only when the
/// file's mtime changes.
pub struct ApiClientStore {
    path: PathBuf,
    cache: Mutex<CacheInner>,
}

pub struct ApiClientFreshGuard {
    pub client: ApiClient,
    pub presented_token_hash: String,
    _registry_lock: std::fs::File,
}

/// Fresh privileged-registry acquisition failures. These are dependency
/// failures, not evidence that a credential was revoked or a binding changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshRegistryError {
    Contended,
    Internal,
}

impl ApiClientStore {
    /// Build a store over an explicit path (used by tests).
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Mutex::new(CacheInner {
                mtime: None,
                loaded: false,
                registry: ApiClientRegistry::default(),
                problem: None,
            }),
        }
    }

    /// Build a store over `config_dir()/api-clients.json`.
    pub fn at_config_dir() -> Option<Self> {
        crate::config::config_dir().map(|d| Self::new(d.join(REGISTRY_FILENAME)))
    }

    /// Registry file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the current registry, reloading from disk only if the file's
    /// mtime changed since the last load (or it was never loaded).
    fn current(&self) -> Result<RegistrySnapshot, ApiError> {
        let disk_mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        let mut cache = self.cache.lock().map_err(|_| {
            ApiError::Internal("API auth registry cache lock is poisoned".to_string())
        })?;
        if !cache.loaded || cache.mtime != disk_mtime {
            let snapshot = load_registry(&self.path);
            cache.registry = snapshot.registry;
            cache.problem = snapshot.problem;
            cache.mtime = disk_mtime;
            cache.loaded = true;
        }
        Ok(RegistrySnapshot {
            registry: cache.registry.clone(),
            problem: cache.problem.clone(),
        })
    }

    /// Look up the client owning `presented` (read-through). Returns the client
    /// only if it matches, is not revoked, and is not expired. The hash compare
    /// is constant-time per candidate.
    pub fn authenticate(&self, presented: &str) -> Result<Option<ApiClient>, ApiError> {
        let registry = self.current()?.registry;
        let presented_hash = hash_token(presented);
        let now = chrono::Utc::now();
        Ok(registry.clients.into_iter().find(|c| {
            !c.revoked && constant_time_eq(&c.token_hash, &presented_hash) && !is_expired(c, now)
        }))
    }

    fn acquire_fresh_registry_lock(&self) -> Result<(PathBuf, std::fs::File), FreshRegistryError> {
        let parent = self
            .path
            .parent()
            .ok_or(FreshRegistryError::Internal)?
            .to_path_buf();
        let lock = open_registry_lock(&parent).map_err(|_| FreshRegistryError::Internal)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(error) => {
                    let error: std::io::Error = error.into();
                    if error.kind() != std::io::ErrorKind::WouldBlock {
                        return Err(FreshRegistryError::Internal);
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(FreshRegistryError::Contended);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        revalidate_registry_lock(&parent, &lock).map_err(|_| FreshRegistryError::Internal)?;
        Ok((parent, lock))
    }

    fn read_fresh_registry(
        &self,
    ) -> Result<Option<(ApiClientRegistry, std::fs::File)>, FreshRegistryError> {
        let (_parent, lock) = self.acquire_fresh_registry_lock()?;
        let (bytes, _) =
            match crate::path_identity::read_bounded_regular(&self.path, REGISTRY_MAX_BYTES) {
                Ok(value) => value,
                Err(code)
                    if code == "unsafe_path"
                        && matches!(
                            std::fs::symlink_metadata(&self.path),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound
                        ) =>
                {
                    return Ok(None);
                }
                Err(_) => return Err(FreshRegistryError::Internal),
            };
        let value = crate::path_identity::parse_json_no_duplicates(&bytes)
            .map_err(|_| FreshRegistryError::Internal)?;
        let registry: ApiClientRegistry =
            serde_json::from_value(value).map_err(|_| FreshRegistryError::Internal)?;
        validate_registry_strict(&registry).map_err(|_| FreshRegistryError::Internal)?;
        Ok(Some((registry, lock)))
    }

    /// Fresh, cross-process-locked authentication for privileged PTY input.
    /// The ordinary mtime cache is deliberately bypassed.
    pub fn authenticate_pty_input_fresh(
        &self,
        presented: &str,
    ) -> Result<Option<ApiClientFreshGuard>, FreshRegistryError> {
        let Some((registry, lock)) = self.read_fresh_registry()? else {
            return Ok(None);
        };
        let presented_token_hash = hash_token(presented);
        let now = chrono::Utc::now();
        let client = registry.clients.into_iter().find(|client| {
            !client.revoked
                && !is_expired(client, now)
                && constant_time_eq(&client.token_hash, &presented_token_hash)
        });
        Ok(client.map(|client| ApiClientFreshGuard {
            client,
            presented_token_hash,
            _registry_lock: lock,
        }))
    }

    pub async fn authenticate_pty_input_fresh_offloaded(
        self: &std::sync::Arc<Self>,
        presented: String,
    ) -> Result<Option<ApiClientFreshGuard>, FreshRegistryError> {
        let store = std::sync::Arc::clone(self);
        tokio::task::spawn_blocking(move || store.authenticate_pty_input_fresh(&presented))
            .await
            .map_err(|_| FreshRegistryError::Internal)?
    }

    pub fn load_active_binding_fresh(
        &self,
        client_id: &str,
        generation: &str,
    ) -> Result<Option<ApiClientFreshGuard>, FreshRegistryError> {
        let Some((registry, lock)) = self.read_fresh_registry()? else {
            return Ok(None);
        };
        let now = chrono::Utc::now();
        let client = registry.clients.into_iter().find(|client| {
            client.client_id == client_id
                && client.credential_generation.as_deref() == Some(generation)
                && !client.revoked
                && !is_expired(client, now)
                && client.has_scope(SCOPE_PTY_INPUT)
        });
        Ok(client.map(|client| ApiClientFreshGuard {
            presented_token_hash: client.token_hash.clone(),
            client,
            _registry_lock: lock,
        }))
    }

    pub async fn load_active_binding_fresh_offloaded(
        self: &std::sync::Arc<Self>,
        client_id: String,
        generation: String,
    ) -> Result<Option<ApiClientFreshGuard>, FreshRegistryError> {
        let store = std::sync::Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            store.load_active_binding_fresh(&client_id, &generation)
        })
        .await
        .map_err(|_| FreshRegistryError::Internal)?
    }
}

/// Outcome of a successful mint: the plaintext secret (shown ONCE) + the id.
pub struct MintOutcome {
    pub client_id: String,
    pub secret: String,
}

/// Validate a requested scope set against the increment-1 allowlist.
pub fn validate_scopes(scopes: &[String]) -> Result<(), String> {
    if scopes.is_empty() {
        return Err("at least one scope is required".to_string());
    }
    for s in scopes {
        if !VALID_SCOPES.contains(&s.as_str()) {
            return Err(format!(
                "unknown scope '{}'; valid scopes: {}",
                s,
                VALID_SCOPES.join(", ")
            ));
        }
    }
    Ok(())
}

/// Parameters for [`mint`], bundled to keep the call arity low. `client_id` /
/// `secret` / `issued_at` are injected so the caller owns randomness and the
/// timestamp (the CLI passes fresh UUIDs and `Utc::now()`).
pub struct MintRequest {
    pub client_id: String,
    pub secret: String,
    pub label: String,
    pub bound_root: String,
    pub bound_fqn: String,
    pub scopes: Vec<String>,
    pub issued_at: String,
    pub expires_at: Option<String>,
    pub bound_session_id: Option<String>,
    pub credential_generation: Option<String>,
}

fn automatic_client(client: &ApiClient) -> bool {
    if client.bound_session_id.is_some() {
        return true;
    }
    let Some(session_id) = client.client_id.strip_prefix("container-") else {
        return false;
    };
    let Ok(parsed) = Uuid::parse_str(session_id) else {
        return false;
    };
    parsed.get_version_num() == 4
        && parsed.to_string() == session_id
        && client.label == format!("container:{session_id}")
}

fn compact_inert_automatic_clients(registry: &mut ApiClientRegistry) {
    let now = chrono::Utc::now();
    let mut retained_witness = false;
    registry.clients.retain(|client| {
        if !automatic_client(client) || (!client.revoked && !is_expired(client, now)) {
            return true;
        }
        if retained_witness {
            false
        } else {
            retained_witness = true;
            true
        }
    });
}

/// Mint a new client bound to `bound_root`, persisting atomically. Returns the
/// plaintext secret to show once.
pub fn mint(path: &Path, req: MintRequest) -> Result<MintOutcome, String> {
    validate_scopes(&req.scopes)?;
    if req.bound_session_id.is_some() != req.credential_generation.is_some() {
        return Err(
            "bound session and credential generation must be populated together".to_string(),
        );
    }
    let client = ApiClient {
        client_id: req.client_id.clone(),
        label: req.label,
        token_hash: hash_token(&req.secret),
        bound_fqn: req.bound_fqn,
        bound_root: req.bound_root,
        scopes: req.scopes,
        issued_at: req.issued_at,
        expires_at: req.expires_at,
        revoked: false,
        bound_session_id: req.bound_session_id,
        credential_generation: req.credential_generation,
    };
    write_registry(path, |reg| {
        if let Some(session_id) = client.bound_session_id.as_deref() {
            let legacy_id = format!("container-{session_id}");
            for existing in &mut reg.clients {
                if existing.bound_session_id.as_deref() == Some(session_id)
                    || existing.client_id == legacy_id
                {
                    existing.revoked = true;
                }
            }
            compact_inert_automatic_clients(reg);
        }
        if reg.clients.len() >= REGISTRY_MAX_CLIENTS {
            return Err("api_registry_capacity".to_string());
        }
        reg.clients.push(client.clone());
        Ok(())
    })?;
    Ok(MintOutcome {
        client_id: req.client_id,
        secret: req.secret,
    })
}

/// Mark a client revoked (idempotent). Returns whether a matching client was
/// found. Persists atomically.
pub fn revoke(path: &Path, client_id: &str) -> Result<bool, String> {
    let mut found = false;
    write_registry(path, |reg| {
        for c in reg.clients.iter_mut() {
            if c.client_id == client_id {
                c.revoked = true;
                found = true;
            }
        }
        Ok(())
    })?;
    Ok(found)
}

/// List clients (secrets/hashes are the caller's concern to redact).
pub fn list(path: &Path) -> ApiClientRegistry {
    load_registry(path).registry
}

/// List clients with registry load diagnostics for operator-facing commands.
pub fn list_with_status(path: &Path) -> RegistrySnapshot {
    load_registry(path)
}

const REGISTRY_MAX_BYTES: usize = 4 * 1024 * 1024;
const REGISTRY_MAX_CLIENTS: usize = 4_096;

fn open_registry_lock(parent: &Path) -> Result<std::fs::File, String> {
    crate::path_identity::verify_directory(parent)?;
    let path = parent.join("api-clients.lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0x0002_0000);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(&path)
        .map_err(|_| "api_registry_lock_failed".to_string())?;
    crate::path_identity::verify_opened_regular_file(&path, &file, true)
        .map_err(|_| "api_registry_lock_failed".to_string())?;
    Ok(file)
}

fn revalidate_registry_lock(parent: &Path, file: &std::fs::File) -> Result<(), String> {
    crate::path_identity::verify_directory(parent)
        .map_err(|_| "api_registry_lock_failed".to_string())?;
    crate::path_identity::verify_opened_regular_file(&parent.join("api-clients.lock"), file, true)
        .map(|_| ())
        .map_err(|_| "api_registry_lock_failed".to_string())
}

fn validate_registry_strict(registry: &ApiClientRegistry) -> Result<(), String> {
    if registry.version != 1 || registry.clients.len() > REGISTRY_MAX_CLIENTS {
        return Err("api_registry_invalid".to_string());
    }
    let mut ids = std::collections::HashSet::new();
    let mut hashes = std::collections::HashSet::new();
    let mut generations = std::collections::HashSet::new();
    for client in &registry.clients {
        let hash = client.token_hash.strip_prefix("sha256:");
        if client.client_id.is_empty()
            || !ids.insert(client.client_id.as_str())
            || !hashes.insert(client.token_hash.as_str())
            || client.bound_session_id.is_some() != client.credential_generation.is_some()
            || !hash.is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            || validate_scopes(&client.scopes).is_err()
        {
            return Err("api_registry_invalid".to_string());
        }
        if let (Some(session_id), Some(generation)) = (
            client.bound_session_id.as_deref(),
            client.credential_generation.as_deref(),
        ) {
            if crate::phone::types::parse_canonical_uuid_v4(session_id).is_err()
                || crate::phone::types::parse_canonical_uuid_v4(generation).is_err()
                || !generations.insert(generation)
            {
                return Err("api_registry_invalid".to_string());
            }
        }
    }
    Ok(())
}

fn read_registry_strict(path: &Path) -> Result<ApiClientRegistry, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ApiClientRegistry::default());
        }
        Err(_) => return Err("api_registry_invalid".to_string()),
    }
    let (bytes, _) = crate::path_identity::read_bounded_regular(path, REGISTRY_MAX_BYTES)
        .map_err(|_| "api_registry_invalid".to_string())?;
    let value = crate::path_identity::parse_json_no_duplicates(&bytes)
        .map_err(|_| "api_registry_invalid".to_string())?;
    let registry: ApiClientRegistry =
        serde_json::from_value(value).map_err(|_| "api_registry_invalid".to_string())?;
    validate_registry_strict(&registry)?;
    Ok(registry)
}

fn write_registry_bytes(path: &Path, registry: &ApiClientRegistry) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(registry).map_err(|_| "api_registry_write_failed".to_string())?;
    if bytes.len() > REGISTRY_MAX_BYTES {
        return Err("api_registry_capacity".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "api_registry_write_failed".to_string())?;
    let parent_identity = crate::path_identity::verify_directory(parent)
        .map_err(|_| "api_registry_write_failed".to_string())?;
    let destination_identity = match std::fs::symlink_metadata(path) {
        Ok(_) => Some(
            crate::path_identity::verify_regular_file(path)
                .map_err(|_| "api_registry_write_failed".to_string())?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("api_registry_write_failed".to_string()),
    };
    let temp = parent.join(format!(".api-clients-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0x0002_0000);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options
        .open(&temp)
        .map_err(|_| "api_registry_write_failed".to_string())?;
    let publish = (|| {
        file.write_all(&bytes)
            .map_err(|_| "api_registry_write_failed".to_string())?;
        file.flush()
            .map_err(|_| "api_registry_write_failed".to_string())?;
        file.sync_all()
            .map_err(|_| "api_registry_write_failed".to_string())?;
        let temp_identity = crate::path_identity::verify_opened_regular_file(&temp, &file, false)
            .map_err(|_| "api_registry_write_failed".to_string())?;
        drop(file);
        let current_parent = crate::path_identity::verify_directory(parent)
            .map_err(|_| "api_registry_write_failed".to_string())?;
        let current_temp = crate::path_identity::verify_regular_file(&temp)
            .map_err(|_| "api_registry_write_failed".to_string())?;
        if !crate::path_identity::same_object(&parent_identity, &current_parent)
            || !crate::path_identity::same_object(&temp_identity, &current_temp)
        {
            return Err("api_registry_write_failed".to_string());
        }
        match &destination_identity {
            Some(expected) => {
                let current = crate::path_identity::verify_regular_file(path)
                    .map_err(|_| "api_registry_write_failed".to_string())?;
                if !crate::path_identity::same_object(expected, &current) {
                    return Err("api_registry_write_failed".to_string());
                }
            }
            None => match std::fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("api_registry_write_failed".to_string()),
            },
        }
        crate::config::root_agent::atomic_replace_existing(&temp, path)
            .map_err(|_| "api_registry_write_failed".to_string())?;
        let (published, _) = crate::path_identity::read_bounded_regular(path, REGISTRY_MAX_BYTES)
            .map_err(|_| "api_registry_write_failed".to_string())?;
        if published != bytes {
            return Err("api_registry_write_failed".to_string());
        }
        let published_parent = crate::path_identity::verify_directory(parent)
            .map_err(|_| "api_registry_write_failed".to_string())?;
        if !crate::path_identity::same_object(&parent_identity, &published_parent) {
            return Err("api_registry_write_failed".to_string());
        }
        if let Ok(directory) = std::fs::File::open(parent) {
            directory
                .sync_all()
                .map_err(|_| "api_registry_write_failed".to_string())?;
        }
        Ok(())
    })();
    if publish.is_err() {
        if let Err(error) = std::fs::remove_file(&temp) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err("api_registry_write_failed".to_string());
            }
        }
    }
    publish
}

/// Cross-process atomic read-modify-write of the strict bounded registry. The
/// stable dedicated lock remains held through file and parent-directory fsync.
fn write_registry<F>(path: &Path, mutate: F) -> Result<(), String>
where
    F: FnOnce(&mut ApiClientRegistry) -> Result<(), String>,
{
    let parent = path
        .parent()
        .ok_or_else(|| "api_registry_write_failed".to_string())?;
    std::fs::create_dir_all(parent).map_err(|_| "api_registry_write_failed".to_string())?;
    let lock = open_registry_lock(parent)?;
    lock.lock()
        .map_err(|_| "api_registry_lock_failed".to_string())?;
    revalidate_registry_lock(parent, &lock)?;
    let mut registry = read_registry_strict(path)?;
    mutate(&mut registry)?;
    validate_registry_strict(&registry)?;
    write_registry_bytes(path, &registry)
}

// ── Per-source failed-auth lockout (§0.5 DESIGN DECISION) ──────────────────

struct SourceState {
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

/// Unconditional per-source-IP failed-auth lockout. After `threshold` failed
/// auths within `window`, a source is locked for `lockout`. Covers the
/// unauthenticated-traffic gap the per-client rate limit cannot (an attacker
/// with no valid token never reaches the per-client limiter).
pub struct FailedAuthLockout {
    inner: Mutex<HashMap<IpAddr, SourceState>>,
    threshold: usize,
    window: Duration,
    lockout: Duration,
}

impl Default for FailedAuthLockout {
    fn default() -> Self {
        // 10 failed auths in 10s -> 60s lockout.
        Self::new(10, Duration::from_secs(10), Duration::from_secs(60))
    }
}

impl FailedAuthLockout {
    /// Build a lockout with explicit thresholds (tests use tight values).
    pub fn new(threshold: usize, window: Duration, lockout: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            threshold,
            window,
            lockout,
        }
    }

    /// Reject (429) if the source is currently locked. Call before auth.
    pub fn check(&self, ip: IpAddr) -> Result<(), ApiError> {
        self.check_at(ip, Instant::now())
    }

    fn check_at(&self, ip: IpAddr, now: Instant) -> Result<(), ApiError> {
        let map = self.inner.lock().map_err(|_| {
            ApiError::Internal("API failed-auth lockout lock is poisoned".to_string())
        })?;
        if let Some(state) = map.get(&ip) {
            if let Some(until) = state.locked_until {
                if until > now {
                    return Err(ApiError::TooManyRequests(
                        "too many failed authentications from this source; locked out".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Record a failed auth; may transition the source into a lockout.
    pub fn record_failure(&self, ip: IpAddr) -> Result<(), ApiError> {
        self.record_failure_at(ip, Instant::now())
    }

    fn record_failure_at(&self, ip: IpAddr, now: Instant) -> Result<(), ApiError> {
        let mut map = self.inner.lock().map_err(|_| {
            ApiError::Internal("API failed-auth lockout lock is poisoned".to_string())
        })?;
        let state = map.entry(ip).or_insert_with(|| SourceState {
            failures: VecDeque::new(),
            locked_until: None,
        });
        // Drop failures outside the sliding window.
        while let Some(&front) = state.failures.front() {
            if now.duration_since(front) > self.window {
                state.failures.pop_front();
            } else {
                break;
            }
        }
        state.failures.push_back(now);
        if state.failures.len() >= self.threshold {
            state.locked_until = Some(now + self.lockout);
            state.failures.clear();
        }
        Ok(())
    }

    /// Clear a source's failure history after a successful auth.
    pub fn record_success(&self, ip: IpAddr) -> Result<(), ApiError> {
        let mut map = self.inner.lock().map_err(|_| {
            ApiError::Internal("API failed-auth lockout lock is poisoned".to_string())
        })?;
        map.remove(&ip);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn client(hash: &str, revoked: bool, expires: Option<&str>) -> ApiClient {
        ApiClient {
            client_id: "c1".into(),
            label: "l".into(),
            token_hash: hash.into(),
            bound_fqn: "proj:wg-1/dev".into(),
            bound_root: "C:/root".into(),
            scopes: vec![
                SCOPE_SEND.into(),
                SCOPE_LIST_PEERS.into(),
                SCOPE_SESSION_TRANSPORT.into(),
            ],
            issued_at: "2026-01-01T00:00:00Z".into(),
            expires_at: expires.map(|s| s.to_string()),
            revoked,
            bound_session_id: None,
            credential_generation: None,
        }
    }

    #[test]
    fn hash_is_deterministic_and_prefixed() {
        let h = hash_token("secret-abc");
        assert!(h.starts_with("sha256:"));
        assert_eq!(h, hash_token("secret-abc"));
        assert_ne!(h, hash_token("secret-abd"));
        // sha256 hex is 64 chars after the prefix.
        assert_eq!(h.len(), "sha256:".len() + 64);
    }

    #[test]
    fn hash_never_contains_plaintext() {
        let secret = "super-secret-value";
        let h = hash_token(secret);
        assert!(!h.contains(secret));
    }

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }

    #[test]
    fn expiry_fail_closed_on_garbage() {
        let now = chrono::Utc::now();
        assert!(!is_expired(&client("h", false, None), now));
        assert!(is_expired(&client("h", false, Some("not-a-date")), now));
        assert!(is_expired(
            &client("h", false, Some("2000-01-01T00:00:00Z")),
            now
        ));
        assert!(!is_expired(
            &client("h", false, Some("2099-01-01T00:00:00Z")),
            now
        ));
    }

    #[test]
    fn validate_scopes_rejects_unknown_and_empty() {
        assert!(validate_scopes(&[]).is_err());
        assert!(validate_scopes(&["send".into()]).is_ok());
        assert!(validate_scopes(&["send".into(), "list-peers-lean".into()]).is_ok());
        assert!(validate_scopes(&["session-transport".into()]).is_ok());
        assert!(validate_scopes(&["close-session".into()]).is_err());
    }

    #[test]
    fn mint_stores_hash_not_plaintext_and_authenticate_reads_through() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        let out = mint(
            &path,
            MintRequest {
                client_id: "client-1".into(),
                secret: "the-secret".into(),
                label: "docker:test".into(),
                bound_root: "C:/root/replica".into(),
                bound_fqn: "proj:wg-1/dev".into(),
                scopes: vec![SCOPE_SEND.into()],
                issued_at: "2026-01-01T00:00:00Z".into(),
                expires_at: None,
                bound_session_id: None,
                credential_generation: None,
            },
        )
        .unwrap();
        assert_eq!(out.secret, "the-secret");

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("the-secret"),
            "plaintext must not be persisted"
        );
        assert!(raw.contains("sha256:"));

        // A fresh store (empty cache) reads through to disk on first auth.
        let store = ApiClientStore::new(path.clone());
        assert!(store.authenticate("the-secret").unwrap().is_some());
        assert!(store.authenticate("wrong-secret").unwrap().is_none());
    }

    #[test]
    fn revoke_takes_effect_on_next_read_through() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        mint(
            &path,
            MintRequest {
                client_id: "client-1".into(),
                secret: "s".into(),
                label: "l".into(),
                bound_root: "C:/root".into(),
                bound_fqn: "proj:wg-1/dev".into(),
                scopes: vec![SCOPE_SEND.into()],
                issued_at: "2026-01-01T00:00:00Z".into(),
                expires_at: None,
                bound_session_id: None,
                credential_generation: None,
            },
        )
        .unwrap();
        let store = ApiClientStore::new(path.clone());
        assert!(store.authenticate("s").unwrap().is_some());

        // Revoke via the (separate-process-equivalent) file write.
        assert!(revoke(&path, "client-1").unwrap());
        // Read-through picks up the change (mtime advanced).
        assert!(
            store.authenticate("s").unwrap().is_none(),
            "revocation must take effect on the next read-through"
        );
    }

    #[test]
    fn store_authenticate_skips_revoked_and_expired() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        let reg = ApiClientRegistry {
            version: 1,
            clients: vec![
                client(&hash_token("revoked-tok"), true, None),
                client(
                    &hash_token("expired-tok"),
                    false,
                    Some("2000-01-01T00:00:00Z"),
                ),
            ],
        };
        std::fs::write(&path, serde_json::to_string_pretty(&reg).unwrap()).unwrap();
        let store = ApiClientStore::new(path);
        assert!(store.authenticate("revoked-tok").unwrap().is_none());
        assert!(store.authenticate("expired-tok").unwrap().is_none());
    }

    #[test]
    fn privileged_registry_rejects_a_dangling_link_when_supported() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        let missing = dir.path().join("missing-registry.json");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&missing, &path).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&missing, &path).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        if linked {
            assert!(read_registry_strict(&path).is_err());
            let store = ApiClientStore::new(path);
            assert!(store.authenticate_pty_input_fresh("anything").is_err());
        }
    }

    #[test]
    fn api_client_debug_redacts_credential_and_bound_path() {
        let mut value = client(&hash_token("credential-sentinel"), false, None);
        value.bound_root = "C:/bound-path-sentinel".to_string();
        let rendered = format!("{value:?}");
        assert!(!rendered.contains("credential-sentinel"));
        assert!(!rendered.contains(&value.token_hash));
        assert!(!rendered.contains("bound-path-sentinel"));
    }

    #[test]
    fn malformed_registry_snapshot_is_fail_closed_and_visible() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        std::fs::write(&path, "{ invalid").unwrap();

        let snapshot = list_with_status(&path);

        assert!(snapshot.registry.clients.is_empty());
        let problem = snapshot.problem.expect("malformed registry is reported");
        assert_eq!(problem.status, "malformed");
        assert!(problem.message.contains(REGISTRY_FILENAME), "{:?}", problem);
    }

    #[test]
    fn poisoned_registry_cache_returns_internal_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        let store = ApiClientStore::new(path);

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = store.cache.lock().unwrap();
            panic!("poison registry cache");
        }));

        let err = store.authenticate("anything").unwrap_err();
        assert!(matches!(err, ApiError::Internal(_)));
    }

    #[test]
    fn automatic_generation_compaction_preserves_manual_and_one_history_witness() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        mint(
            &path,
            MintRequest {
                client_id: "container-manual-name".into(),
                secret: "manual-secret".into(),
                label: "container:manual".into(),
                bound_root: "C:/manual".into(),
                bound_fqn: "project:wg-1-team/manual".into(),
                scopes: vec![SCOPE_SEND.into()],
                issued_at: chrono::Utc::now().to_rfc3339(),
                expires_at: None,
                bound_session_id: None,
                credential_generation: None,
            },
        )
        .unwrap();
        let session_id = uuid::Uuid::new_v4().to_string();
        for index in 0..3 {
            let generation = uuid::Uuid::new_v4().to_string();
            mint(
                &path,
                MintRequest {
                    client_id: format!("container-{session_id}-{generation}"),
                    secret: format!("automatic-secret-{index}"),
                    label: format!("container:{session_id}"),
                    bound_root: "C:/automatic".into(),
                    bound_fqn: "project:wg-1-team/coordinator".into(),
                    scopes: vec![SCOPE_SEND.into(), SCOPE_PTY_INPUT.into()],
                    issued_at: chrono::Utc::now().to_rfc3339(),
                    expires_at: None,
                    bound_session_id: Some(session_id.clone()),
                    credential_generation: Some(generation),
                },
            )
            .unwrap();
        }
        let registry = read_registry_strict(&path).unwrap();
        assert!(registry
            .clients
            .iter()
            .any(|client| client.client_id == "container-manual-name" && !client.revoked));
        let automatic: Vec<_> = registry
            .clients
            .iter()
            .filter(|client| automatic_client(client))
            .collect();
        assert_eq!(automatic.len(), 2, "one live generation plus one witness");
        assert_eq!(automatic.iter().filter(|client| !client.revoked).count(), 1);
    }

    #[tokio::test]
    async fn privileged_registry_lock_wait_is_offloaded_from_async_ingress() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        let lock = open_registry_lock(dir.path()).unwrap();
        lock.lock().unwrap();
        let store = std::sync::Arc::new(ApiClientStore::new(path));
        let waiting = {
            let store = std::sync::Arc::clone(&store);
            tokio::spawn(async move {
                store
                    .authenticate_pty_input_fresh_offloaded("not-a-secret".to_string())
                    .await
            })
        };

        tokio::time::timeout(Duration::from_millis(100), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        })
        .await
        .expect("the async executor must progress while the file lock is held");
        assert!(matches!(
            waiting.await.unwrap(),
            Err(FreshRegistryError::Contended)
        ));
    }

    #[test]
    fn privileged_registry_contention_is_retry_class_not_stale() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        let lock = open_registry_lock(dir.path()).unwrap();
        lock.lock().unwrap();
        let store = ApiClientStore::new(path);

        let error = match store.authenticate_pty_input_fresh("not-a-secret") {
            Err(error) => error,
            Ok(_) => panic!("a held registry lock must report contention"),
        };

        assert_eq!(
            error,
            FreshRegistryError::Contended,
            "lock contention is retryable and must never be classified as stale authority"
        );
    }

    #[test]
    fn fresh_privileged_auth_observes_revocation_without_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(REGISTRY_FILENAME);
        let session_id = uuid::Uuid::new_v4().to_string();
        let generation = uuid::Uuid::new_v4().to_string();
        mint(
            &path,
            MintRequest {
                client_id: format!("container-{session_id}-{generation}"),
                secret: "fresh-secret".into(),
                label: format!("container:{session_id}"),
                bound_root: "C:/automatic".into(),
                bound_fqn: "project:wg-1-team/coordinator".into(),
                scopes: vec![SCOPE_PTY_INPUT.into()],
                issued_at: chrono::Utc::now().to_rfc3339(),
                expires_at: None,
                bound_session_id: Some(session_id),
                credential_generation: Some(generation),
            },
        )
        .unwrap();
        let store = ApiClientStore::new(path.clone());
        assert!(store
            .authenticate_pty_input_fresh("fresh-secret")
            .unwrap()
            .is_some());
        let client_id = read_registry_strict(&path).unwrap().clients[0]
            .client_id
            .clone();
        assert!(revoke(&path, &client_id).unwrap());
        assert!(store
            .authenticate_pty_input_fresh("fresh-secret")
            .unwrap()
            .is_none());
    }

    #[test]
    fn lockout_triggers_after_threshold_and_check_rejects() {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let lock = FailedAuthLockout::new(3, Duration::from_secs(10), Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(lock.check_at(ip, t0).is_ok());
        lock.record_failure_at(ip, t0).unwrap();
        lock.record_failure_at(ip, t0).unwrap();
        assert!(lock.check_at(ip, t0).is_ok(), "below threshold: allowed");
        lock.record_failure_at(ip, t0).unwrap();
        assert!(
            lock.check_at(ip, t0).is_err(),
            "at threshold: source is locked"
        );
        // After lockout expires, allowed again.
        assert!(lock.check_at(ip, t0 + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn lockout_success_clears_history() {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let lock = FailedAuthLockout::new(3, Duration::from_secs(10), Duration::from_secs(60));
        let t0 = Instant::now();
        lock.record_failure_at(ip, t0).unwrap();
        lock.record_failure_at(ip, t0).unwrap();
        lock.record_success(ip).unwrap();
        lock.record_failure_at(ip, t0).unwrap();
        lock.record_failure_at(ip, t0).unwrap();
        assert!(
            lock.check_at(ip, t0).is_ok(),
            "success reset the counter, so 2 more failures stay below threshold"
        );
    }

    #[test]
    fn poisoned_lockout_returns_internal_error() {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let lock = FailedAuthLockout::default();

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lock.inner.lock().unwrap();
            panic!("poison lockout");
        }));

        let err = lock.check(ip).unwrap_err();
        assert!(matches!(err, ApiError::Internal(_)));
        let err = lock.record_failure(ip).unwrap_err();
        assert!(matches!(err, ApiError::Internal(_)));
        let err = lock.record_success(ip).unwrap_err();
        assert!(matches!(err, ApiError::Internal(_)));
    }
}
