//! Persisted idempotency ledger for `send` (#791 §6.3, §0.5 G6).
//!
//! `send` is not safely repeatable (a duplicate would double-wake the peer), so
//! a client-generated `opId` keys a bounded, TTL'd, DISK-PERSISTED ledger of
//! `opId -> result`. A replay returns the SAME stored result and never
//! re-delivers, and the persistence means an API-server restart cannot reset
//! dedup and double-deliver. The daemon (API server) is the sole writer, so no
//! cross-process reload is needed; the file is reloaded on construction.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Default retention window: replays older than this are pruned (and hence
/// re-deliverable). Bounds the ledger's time horizon.
const DEFAULT_TTL_SECS: i64 = 24 * 60 * 60;
/// Default max entries retained (oldest evicted first). Bounds the file size.
const DEFAULT_CAP: usize = 2000;
/// Ledger file basename in `config_dir()`.
pub const LEDGER_FILENAME: &str = "api-idempotency.json";

/// A stored `send` result, replayed verbatim on a duplicate `opId`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredResult {
    pub op_id: String,
    /// `"delivered"` | `"rejected"`.
    pub status: String,
    pub to: String,
    #[serde(default)]
    pub detail: Option<String>,
    /// RFC3339 first-seen timestamp (for TTL pruning).
    pub first_seen: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LedgerDoc {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<StoredResult>,
}

/// Disk-persisted, bounded, TTL'd replay ledger.
pub struct IdempotencyLedger {
    path: PathBuf,
    ttl_secs: i64,
    cap: usize,
    entries: Mutex<Vec<StoredResult>>,
}

impl IdempotencyLedger {
    /// Load a ledger from `path` (empty if absent/malformed), with explicit
    /// bounds (tests use tight values).
    pub fn load(path: PathBuf, ttl_secs: i64, cap: usize) -> Self {
        let entries = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<LedgerDoc>(&raw)
                .map(|d| d.entries)
                .unwrap_or_else(|e| {
                    log::warn!(
                        "[api-idempotency] {} malformed ({}); starting empty",
                        LEDGER_FILENAME,
                        e
                    );
                    Vec::new()
                }),
            Err(_) => Vec::new(),
        };
        Self {
            path,
            ttl_secs,
            cap,
            entries: Mutex::new(entries),
        }
    }

    /// Load from `config_dir()/api-idempotency.json` with default bounds.
    pub fn at_config_dir() -> Option<Self> {
        crate::config::config_dir()
            .map(|d| Self::load(d.join(LEDGER_FILENAME), DEFAULT_TTL_SECS, DEFAULT_CAP))
    }

    /// Return a prior result for `op_id` if it is present and not expired.
    pub fn get(&self, op_id: &str) -> Option<StoredResult> {
        self.get_at(op_id, chrono::Utc::now())
    }

    fn get_at(&self, op_id: &str, now: chrono::DateTime<chrono::Utc>) -> Option<StoredResult> {
        let entries = self.entries.lock().unwrap();
        entries
            .iter()
            .find(|e| e.op_id == op_id && !self.is_expired(e, now))
            .cloned()
    }

    /// Record a result for `op_id` (first-write-wins: a duplicate is ignored so
    /// the original result is stable). Prunes expired + over-cap entries and
    /// persists to disk. Best-effort persistence: an I/O failure is logged, not
    /// propagated (the in-memory ledger still dedups within this run).
    pub fn put(&self, op_id: &str, status: &str, to: &str, detail: Option<String>) {
        self.put_at(op_id, status, to, detail, chrono::Utc::now())
    }

    fn put_at(
        &self,
        op_id: &str,
        status: &str,
        to: &str,
        detail: Option<String>,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        let snapshot = {
            let mut entries = self.entries.lock().unwrap();
            if entries.iter().any(|e| e.op_id == op_id) {
                return; // first-write-wins
            }
            entries.push(StoredResult {
                op_id: op_id.to_string(),
                status: status.to_string(),
                to: to.to_string(),
                detail,
                first_seen: now.to_rfc3339(),
            });
            // Prune expired.
            entries.retain(|e| !self.is_expired(e, now));
            // Evict oldest beyond the cap (entries are appended in time order).
            if entries.len() > self.cap {
                let overflow = entries.len() - self.cap;
                entries.drain(0..overflow);
            }
            entries.clone()
        };
        self.persist(&snapshot);
    }

    fn is_expired(&self, entry: &StoredResult, now: chrono::DateTime<chrono::Utc>) -> bool {
        match chrono::DateTime::parse_from_rfc3339(&entry.first_seen) {
            Ok(seen) => now.signed_duration_since(seen.with_timezone(&chrono::Utc))
                > chrono::Duration::seconds(self.ttl_secs),
            // Unparseable stamp: treat as expired so it cannot linger forever.
            Err(_) => true,
        }
    }

    fn persist(&self, entries: &[StoredResult]) {
        let doc = LedgerDoc {
            version: 1,
            entries: entries.to_vec(),
        };
        let result = crate::config::local_config_io::update_config_json_object(
            &self.path,
            true,
            |obj| {
                let new = serde_json::to_value(&doc).map_err(|e| e.to_string())?;
                if let serde_json::Value::Object(map) = new {
                    *obj = map;
                }
                Ok(())
            },
        );
        if let Err(e) = result {
            log::warn!(
                "[api-idempotency] failed to persist ledger (continuing in-memory): {}",
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_get_returns_stored_result() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger = IdempotencyLedger::load(dir.path().join(LEDGER_FILENAME), 3600, 100);
        ledger.put("op-1", "delivered", "proj/agent", None);
        let got = ledger.get("op-1").expect("op-1 present");
        assert_eq!(got.status, "delivered");
        assert_eq!(got.to, "proj/agent");
        assert!(ledger.get("op-2").is_none());
    }

    #[test]
    fn put_is_first_write_wins() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger = IdempotencyLedger::load(dir.path().join(LEDGER_FILENAME), 3600, 100);
        ledger.put("op-1", "delivered", "a", None);
        ledger.put("op-1", "rejected", "b", Some("late".into()));
        let got = ledger.get("op-1").unwrap();
        assert_eq!(got.status, "delivered", "the first result must be stable");
        assert_eq!(got.to, "a");
    }

    #[test]
    fn survives_reload_from_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(LEDGER_FILENAME);
        {
            let ledger = IdempotencyLedger::load(path.clone(), 3600, 100);
            ledger.put("op-1", "delivered", "a", None);
        }
        // A fresh ledger (mirrors an API-server restart) reloads from disk.
        let reloaded = IdempotencyLedger::load(path, 3600, 100);
        assert!(
            reloaded.get("op-1").is_some(),
            "a restart must not reset dedup (§0.5 G6)"
        );
    }

    #[test]
    fn expired_entries_are_not_returned() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger = IdempotencyLedger::load(dir.path().join(LEDGER_FILENAME), 60, 100);
        let now = chrono::Utc::now();
        ledger.put_at("op-1", "delivered", "a", None, now);
        // 61s later, the 60s TTL has elapsed.
        let later = now + chrono::Duration::seconds(61);
        assert!(ledger.get_at("op-1", later).is_none());
    }

    #[test]
    fn cap_evicts_oldest() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger = IdempotencyLedger::load(dir.path().join(LEDGER_FILENAME), 3600, 2);
        let now = chrono::Utc::now();
        ledger.put_at("op-1", "delivered", "a", None, now);
        ledger.put_at("op-2", "delivered", "b", None, now);
        ledger.put_at("op-3", "delivered", "c", None, now);
        assert!(ledger.get_at("op-1", now).is_none(), "oldest evicted");
        assert!(ledger.get_at("op-2", now).is_some());
        assert!(ledger.get_at("op-3", now).is_some());
    }
}
