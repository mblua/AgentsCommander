//! Issue #609 - "npm update available" detection.
//!
//! On startup a detached task (see `lib.rs` setup) reads the baked-in version,
//! optionally queries the npm registry for the latest published version of
//! `@mblua/agentscommander` (throttled to <=1x/24h via an on-disk cache),
//! compares, and on a newer version caches an `UpdateInfo` + emits
//! `npm_update_available`. Everything is fail-silent: any error logs at debug
//! and produces no notification. The task is detached, so startup never blocks.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// The published package. Used verbatim in the upgrade command shown to the
/// user (literal `/`, because that is exactly what they type into a shell).
const PACKAGE_NAME: &str = "@mblua/agentscommander";
/// npm registry dist-tags endpoint. The scope separator MUST be `%2F`-encoded:
/// reqwest sends a literal `/` as a path segment, and the `%2F` form is the
/// unambiguous, npm-CLI-proven shape. Verified live by tech-lead (H1): this URL
/// returns `{"latest":"..."}`. Do NOT switch back to a literal `/`.
const DIST_TAGS_URL: &str =
    "https://registry.npmjs.org/-/package/@mblua%2Fagentscommander/dist-tags";
const CHECK_INTERVAL_HOURS: i64 = 24;
const REQUEST_TIMEOUT_SECS: u64 = 10;
/// Hard cap on the dist-tags response body, mirroring fetch_home_markdown's
/// length-check (L1). The JSON is tiny; 64 KB is a generous bound.
const DIST_TAGS_MAX_BYTES: usize = 64 * 1024;
const CACHE_FILE_NAME: &str = "update-check.json";

/// Cached result of an "update available" computation. Sent to the frontend
/// (serialized camelCase) and stored in `UpdateCheckState`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub upgrade_command: String,
}

/// On-disk throttle cache. `last_checked_at` gates the next network call;
/// `latest_version` lets us still notify on every boot inside the 24h window
/// without re-hitting the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCache {
    last_checked_at: DateTime<Utc>,
    latest_version: String,
}

fn cache_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join(CACHE_FILE_NAME))
}

fn read_cache() -> Option<UpdateCache> {
    let path = cache_path()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<UpdateCache>(&bytes).ok()
}

fn write_cache(cache: &UpdateCache) {
    let Some(path) = cache_path() else { return };
    let Ok(json) = serde_json::to_string_pretty(cache) else { return };
    // Atomic write (L3/E1): write a sibling temp file, then rename over the
    // target so a concurrent reader (the Phase-2 CLI) never observes a
    // half-written file. On Windows, std::fs::rename maps to MoveFileExW with
    // MOVEFILE_REPLACE_EXISTING, so it atomically replaces the existing cache.
    // Best-effort throughout; a failed write/rename only means we re-check
    // sooner.
    let tmp = path.with_extension("json.tmp"); // -> update-check.json.tmp (same dir)
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// True when we should hit the network now: no cache, stale cache (older than
/// the interval), or a cache timestamp in the future (clock moved backwards).
fn should_check(cache: &Option<UpdateCache>, now: DateTime<Utc>) -> bool {
    match cache {
        None => true,
        Some(c) => {
            let age = now.signed_duration_since(c.last_checked_at);
            age >= chrono::Duration::hours(CHECK_INTERVAL_HOURS) || age.num_seconds() < 0
        }
    }
}

// ---- Pure decision logic (T1) ---------------------------------------------
// Extracted so toggle-off / throttle-hit / fetch-fail-fallback /
// write-only-on-success are unit-testable with zero Tauri and zero network.
// `run_startup_check` is the thin async shell that performs the I/O these
// functions decide on.

/// What the startup check should do this run, decided with no I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckPlan {
    /// Toggle off: do nothing at all.
    Skip,
    /// Inside the 24h window: reuse the cached latest, no network.
    UseCached,
    /// No cache, or stale/clock-skewed cache: hit the registry.
    Fetch,
}

/// Pure: settings toggle + current cache + now -> plan.
fn plan_check(enabled: bool, cache: &Option<UpdateCache>, now: DateTime<Utc>) -> CheckPlan {
    if !enabled {
        CheckPlan::Skip
    } else if should_check(cache, now) {
        CheckPlan::Fetch
    } else {
        CheckPlan::UseCached
    }
}

/// Pure: plan + fetch outcome + existing cache -> (latest to compare against,
/// write_back?). `fetched` is `Some` only on a successful network call.
/// `write_back` is true ONLY on a fresh successful fetch (write-only-on-success);
/// the caller stamps `now` and writes when it is true.
fn resolve_latest(
    plan: CheckPlan,
    fetched: Option<String>,
    cache: &Option<UpdateCache>,
) -> (Option<String>, bool) {
    match plan {
        CheckPlan::Skip => (None, false),
        CheckPlan::UseCached => (cache.as_ref().map(|c| c.latest_version.clone()), false),
        CheckPlan::Fetch => match fetched {
            // Fresh value: compare against it AND signal a cache write.
            Some(v) => (Some(v), true),
            // Fetch failed: fall back to any cached value, no write.
            None => (cache.as_ref().map(|c| c.latest_version.clone()), false),
        },
    }
}

/// Parse a version string into a (major, minor, patch) tuple, ignoring any
/// pre-release/build suffix (everything from the first `-` or `+`). Missing
/// components default to 0. Non-numeric components make the parse fail.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().split(['-', '+']).next().unwrap_or("");
    let mut it = core.split('.');
    let major = it.next()?.parse::<u64>().ok()?;
    let minor = it.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = it.next().unwrap_or("0").parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// True when `latest` is strictly newer than `current`. Fail-closed: if either
/// side fails to parse, returns false (no false-positive notification).
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Pull `dist-tags.latest` out of the registry JSON body. Fail-silent.
/// An empty `latest` (e.g. a malformed `{"latest":""}`) is treated as no result
/// (FIX-2) so we re-check next boot instead of caching `""` and anchoring the
/// 24h throttle on junk.
fn parse_latest(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("latest")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn upgrade_command() -> String {
    format!("npm i -g {}", PACKAGE_NAME)
}

/// Detached startup task. Reads the toggle, applies the 24h throttle, queries
/// the registry only when due, compares, and on a newer version caches the
/// `UpdateInfo` and emits `npm_update_available`. Never returns an error to the
/// caller: own no-ops log at debug; a registry that responds with a bad/
/// unparseable body logs at warn (see `fetch_latest`, H1); everything is
/// swallowed so startup is never affected.
pub async fn run_startup_check(app: AppHandle, cache_state: Arc<OnceLock<UpdateInfo>>) {
    // 1. Read the settings toggle (default ON) once.
    let enabled = {
        let settings_state = app.state::<crate::config::settings::SettingsState>();
        let s = settings_state.read().await;
        s.npm_update_notifications_enabled
    };

    let current = env!("CARGO_PKG_VERSION").to_string();
    let now = Utc::now();
    let cache = read_cache();

    // 2. Pure decision: skip / use-cached / fetch (T1, unit-tested).
    let plan = plan_check(enabled, &cache, now);
    if plan == CheckPlan::Skip {
        log::debug!("[update-check] disabled by settings; skipping");
        return;
    }

    // 3. Hit the network only when the plan says to. Inside the 24h window we
    //    reuse the cached latest version so the toast still appears without
    //    re-hitting the registry.
    let fetched = if plan == CheckPlan::Fetch {
        fetch_latest(&app).await
    } else {
        None
    };

    // 4. Pure resolution + write-only-on-success (T1, unit-tested). On a fresh
    //    successful fetch we stamp `now` and write the cache; on a throttle hit
    //    or a failed fetch we fall back to the cached value and write nothing.
    let (latest, write_back) = resolve_latest(plan, fetched, &cache);
    if write_back {
        if let Some(ref v) = latest {
            write_cache(&UpdateCache {
                last_checked_at: now,
                latest_version: v.clone(),
            });
        }
    }

    let Some(latest) = latest else {
        log::debug!("[update-check] no latest version available; no-op");
        return;
    };

    // 5. Compare + notify.
    if is_newer(&latest, &current) {
        let info = UpdateInfo {
            current_version: current,
            latest_version: latest,
            upgrade_command: upgrade_command(),
        };
        let _ = cache_state.set(info.clone());
        let _ = app.emit("npm_update_available", &info);
        log::info!(
            "[update-check] update available: {} -> {}",
            info.current_version,
            info.latest_version
        );
    } else {
        log::debug!("[update-check] up to date (current {}, latest {})", current, latest);
    }
}

/// GET the dist-tags and return `latest`. Fail-silent on offline/timeout
/// (returns None quietly). Logs at WARN (H1) when the registry *responds* but
/// the response is unusable (non-2xx, empty/oversized, or unparseable) so a
/// wrong URL or a future registry-shape change leaves a breadcrumb in app.log.
/// This warn-on-bad-response posture is the safety net for the no-test-gate
/// path: plain offline stays silent, a broken contract is loud.
async fn fetch_latest(app: &AppHandle) -> Option<String> {
    let network = app.state::<crate::network::OutboundNetwork>();
    let _permit = network.acquire("update_check.dist_tags").await.ok()?;
    let resp = tokio::time::timeout(
        Duration::from_secs(REQUEST_TIMEOUT_SECS),
        network
            .general()
            .get(DIST_TAGS_URL)
            .header(
                reqwest::header::USER_AGENT,
                concat!("agentscommander/", env!("CARGO_PKG_VERSION")),
            )
            .send(),
    )
    .await
    .ok()? // timeout: fail-silent (treat as offline)
    .ok()?; // transport error: fail-silent (treat as offline)

    if !resp.status().is_success() {
        log::warn!(
            "[update-check] registry returned status {}",
            resp.status().as_u16()
        );
        return None;
    }

    // L1: mirror fetch_home_markdown exactly - read bytes and length-check
    // before allocating/decoding a String.
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > DIST_TAGS_MAX_BYTES {
        log::warn!(
            "[update-check] registry body empty or too large ({} bytes)",
            bytes.len()
        );
        return None;
    }
    // Registry returns clean UTF-8 JSON (no BOM); add a strip_prefix for the BOM
    // only if one is ever observed.
    let body = String::from_utf8(bytes.to_vec()).ok()?;
    match parse_latest(&body) {
        Some(v) => Some(v),
        None => {
            log::warn!("[update-check] could not parse 'latest' from dist-tags response");
            None
        }
    }
}

// ---- Phase 2 (in this PR per O1): CLI cache-only notice -------------------

/// Pure: format the CLI notice from the loaded cache + running version + toggle.
/// Returns the one-line string ONLY when notifications are enabled AND a newer
/// version is cached. Kept I/O-free so the toggle-OFF and not-newer paths are
/// unit-testable (mirrors the `plan_check`/`resolve_latest` split);
/// `read_cached_notice` is the thin shell that loads the cache + setting.
fn cli_notice(cache: &UpdateCache, current: &str, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }
    if !is_newer(&cache.latest_version, current) {
        return None;
    }
    Some(format!(
        "Update available: v{} (you have v{}). Run: {}",
        cache.latest_version,
        current,
        upgrade_command()
    ))
}

/// Cache-first check for the CLI startup line. No network. Returns a one-line
/// notice when the cached latest is newer than the running build AND the user
/// has not disabled `npm_update_notifications_enabled`. Safe to call
/// synchronously from `main.rs`.
///
/// FIX-1 (tech-lead): the single opt-out must silence BOTH the GUI toast and
/// this CLI line. This intentionally OVERRIDES plan §5's "no settings read":
/// opt-out consistency wins. To keep the read off every other CLI verb, we load
/// the setting ONLY after confirming a newer version is actually cached (the
/// caller in `main.rs` has already confirmed an interactive stderr). We use the
/// read-only `load_settings_for_cli` loader so the CLI never writes settings.json
/// (the GUI `load_settings` would auto-gen root_token + save, violating the
/// read-only-CLI contract).
pub fn read_cached_notice() -> Option<String> {
    let cache = read_cache()?;
    let current = env!("CARGO_PKG_VERSION");
    // Cheap path: bail before any settings read when nothing newer is cached.
    if !is_newer(&cache.latest_version, current) {
        return None;
    }
    let enabled =
        crate::config::settings::load_settings_for_cli().npm_update_notifications_enabled;
    cli_notice(&cache, current, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_basic() {
        assert!(is_newer("0.9.17", "0.9.16"));
        assert!(is_newer("1.0.0", "0.9.16"));
        assert!(is_newer("0.10.0", "0.9.16"));
        assert!(!is_newer("0.9.16", "0.9.16"));
        assert!(!is_newer("0.9.15", "0.9.16"));
    }

    #[test]
    fn is_newer_ignores_prerelease_and_build() {
        assert!(is_newer("1.0.0-rc.1", "0.9.16")); // numeric core wins
        assert!(!is_newer("0.9.16+build.5", "0.9.16"));
    }

    #[test]
    fn is_newer_fails_closed_on_garbage() {
        assert!(!is_newer("not-a-version", "0.9.16"));
        assert!(!is_newer("0.9.17", "garbage"));
        assert!(!is_newer("", ""));
    }

    #[test]
    fn parse_latest_extracts_tag() {
        assert_eq!(
            parse_latest(r#"{"latest":"1.2.3","beta":"2.0.0-rc.1"}"#),
            Some("1.2.3".to_string())
        );
        assert_eq!(parse_latest(r#"{"no-latest":"x"}"#), None);
        assert_eq!(parse_latest("not json"), None);
        // FIX-2: an empty `latest` is treated as no-result, not cached as "".
        assert_eq!(parse_latest(r#"{"latest":""}"#), None);
    }

    #[test]
    fn cli_notice_honors_toggle_and_version() {
        let newer = UpdateCache {
            last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(),
            latest_version: "0.9.20".into(),
        };
        let same = UpdateCache {
            last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(),
            latest_version: "0.9.16".into(),
        };
        // Enabled + newer -> notice carrying the version + upgrade command.
        let notice = cli_notice(&newer, "0.9.16", true).expect("expected a notice");
        assert!(notice.contains("0.9.20"));
        assert!(notice.contains("npm i -g @mblua/agentscommander"));
        // FIX-1: toggle OFF -> silent even with a newer cache.
        assert_eq!(cli_notice(&newer, "0.9.16", false), None);
        // Enabled but not newer -> silent.
        assert_eq!(cli_notice(&same, "0.9.16", true), None);
    }

    #[test]
    fn should_check_gates_on_age() {
        let now = "2026-06-23T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(should_check(&None, now));
        let fresh = UpdateCache {
            last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(), // 6h ago
            latest_version: "0.9.16".into(),
        };
        assert!(!should_check(&Some(fresh), now));
        let stale = UpdateCache {
            last_checked_at: "2026-06-22T06:00:00Z".parse().unwrap(), // 30h ago
            latest_version: "0.9.16".into(),
        };
        assert!(should_check(&Some(stale), now));
        let future = UpdateCache {
            last_checked_at: "2026-06-24T06:00:00Z".parse().unwrap(), // clock skew
            latest_version: "0.9.16".into(),
        };
        assert!(should_check(&Some(future), now));
    }

    // ---- T1: pure decision-logic tests (zero Tauri, zero network) ----------

    #[test]
    fn plan_check_skip_use_fetch() {
        let now = "2026-06-23T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let fresh = UpdateCache {
            last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(), // 6h ago
            latest_version: "0.9.16".into(),
        };
        // Toggle off -> Skip, even with a fresh cache.
        assert_eq!(plan_check(false, &Some(fresh.clone()), now), CheckPlan::Skip);
        // Enabled + no cache -> Fetch.
        assert_eq!(plan_check(true, &None, now), CheckPlan::Fetch);
        // Enabled + fresh cache (throttle hit) -> UseCached.
        assert_eq!(plan_check(true, &Some(fresh), now), CheckPlan::UseCached);
        // Enabled + stale cache (30h) -> Fetch.
        let stale = UpdateCache {
            last_checked_at: "2026-06-22T06:00:00Z".parse().unwrap(),
            latest_version: "0.9.16".into(),
        };
        assert_eq!(plan_check(true, &Some(stale), now), CheckPlan::Fetch);
    }

    #[test]
    fn resolve_latest_all_paths() {
        let cache = Some(UpdateCache {
            last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(),
            latest_version: "0.9.10".into(),
        });
        // Skip: nothing known, no write.
        assert_eq!(resolve_latest(CheckPlan::Skip, None, &cache), (None, false));
        // UseCached: cached value, no write (throttle hit).
        assert_eq!(
            resolve_latest(CheckPlan::UseCached, None, &cache),
            (Some("0.9.10".into()), false)
        );
        // Fetch success: fresh value, write-back true (write-only-on-success).
        assert_eq!(
            resolve_latest(CheckPlan::Fetch, Some("0.9.20".into()), &cache),
            (Some("0.9.20".into()), true)
        );
        // Fetch fail: fall back to cache, no write.
        assert_eq!(
            resolve_latest(CheckPlan::Fetch, None, &cache),
            (Some("0.9.10".into()), false)
        );
    }

    #[test]
    fn resolve_latest_fetch_fail_no_cache() {
        // Fetch fail with no cache -> nothing known, no write (no false toast).
        assert_eq!(resolve_latest(CheckPlan::Fetch, None, &None), (None, false));
    }
}
