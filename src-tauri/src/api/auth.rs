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
use std::io::ErrorKind;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::ApiError;

/// The `send` scope (POST /api/v1/send).
pub const SCOPE_SEND: &str = "send";
/// The `list-peers-lean` scope (GET /api/v1/peers).
pub const SCOPE_LIST_PEERS: &str = "list-peers-lean";
/// The container session transport scope (GET /api/v1/session-transport).
pub const SCOPE_SESSION_TRANSPORT: &str = "session-transport";
/// The only scopes mintable in increment 1.
pub const VALID_SCOPES: &[&str] = &[SCOPE_SEND, SCOPE_LIST_PEERS, SCOPE_SESSION_TRANSPORT];

/// Registry file basename in `config_dir()`.
pub const REGISTRY_FILENAME: &str = "api-clients.json";

/// One registered API client. `boundRoot` is the identity source (the `from` is
/// derived from it at request time); `boundFqn` is an audit hint only (§0.5 G5).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

impl ApiClient {
    /// Whether this client holds the given scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// On-disk registry document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiClientRegistry {
    pub version: u32,
    #[serde(default)]
    pub clients: Vec<ApiClient>,
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
#[derive(Debug, Clone)]
pub struct RegistrySnapshot {
    pub registry: ApiClientRegistry,
    pub problem: Option<RegistryLoadProblem>,
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
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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
}

/// Mint a new client bound to `bound_root`, persisting atomically. Returns the
/// plaintext secret to show once.
pub fn mint(path: &Path, req: MintRequest) -> Result<MintOutcome, String> {
    validate_scopes(&req.scopes)?;
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
    };
    write_registry(path, |reg| {
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

/// Atomic read-modify-write of the whole typed registry, reusing the process-
/// wide-locked `update_config_json_object` primitive (temp + `ReplaceFileW` /
/// rename publish).
fn write_registry<F>(path: &Path, mutate: F) -> Result<(), String>
where
    F: FnOnce(&mut ApiClientRegistry) -> Result<(), String>,
{
    crate::config::local_config_io::update_config_json_object(path, true, |obj| {
        let mut reg: ApiClientRegistry =
            serde_json::from_value(serde_json::Value::Object(obj.clone()))
                .unwrap_or_default();
        if reg.version == 0 {
            reg.version = 1;
        }
        mutate(&mut reg)?;
        let new = serde_json::to_value(&reg).map_err(|e| e.to_string())?;
        if let serde_json::Value::Object(map) = new {
            *obj = map;
        }
        Ok(())
    })
    .map(|_| ())
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
            scopes: vec![SCOPE_SEND.into(), SCOPE_LIST_PEERS.into(), SCOPE_SESSION_TRANSPORT.into()],
            issued_at: "2026-01-01T00:00:00Z".into(),
            expires_at: expires.map(|s| s.to_string()),
            revoked,
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
            },
        )
        .unwrap();
        assert_eq!(out.secret, "the-secret");

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("the-secret"), "plaintext must not be persisted");
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
                client(&hash_token("expired-tok"), false, Some("2000-01-01T00:00:00Z")),
            ],
        };
        std::fs::write(&path, serde_json::to_string_pretty(&reg).unwrap()).unwrap();
        let store = ApiClientStore::new(path);
        assert!(store.authenticate("revoked-tok").unwrap().is_none());
        assert!(store.authenticate("expired-tok").unwrap().is_none());
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
