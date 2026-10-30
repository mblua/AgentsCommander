//! Issue #609 - "npm update available" detection.
//!
//! On startup a detached task (see `lib.rs` setup) reads the baked-in version,
//! optionally queries the npm registry for the latest published version of
//! `@mblua/agentscommander` (throttled to <=1x/24h via an on-disk cache),
//! compares, and on a newer version caches an `UpdateInfo` + emits
//! `npm_update_available`. Everything is fail-silent: any error logs at debug
//! and produces no notification. The task is detached, so startup never blocks.
//!
//! #1925 - the same detached, fail-silent startup shape also downloads the remote
//! blocking-menu patterns (run_remote_blocking_menus_startup); that download never
//! notifies and applies at the next start.

use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use crate::filesystem::update_cache::Cache as UpdateCache;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use tauri::Manager;

pub trait UpdateEvents {
    fn npm_update_available(&self, info: UpdateInfo);
}

pub trait UpdateCheckHost: UpdateEvents + Send + Sync {
    fn update_settings(&self) -> crate::config::settings::SettingsState;
    fn outbound_network(&self) -> crate::network::OutboundNetwork;
}

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

/// Cached result of an "update available" computation. Sent to the frontend
/// (serialized camelCase) and stored in `UpdateCheckState`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub upgrade_command: String,
}

fn read_cache() -> Option<UpdateCache> {
    let directory = crate::config::config_dir();
    crate::filesystem::update_cache::read(directory.as_deref())
}

fn write_cache(cache: &UpdateCache) {
    let directory = crate::config::config_dir();
    crate::filesystem::update_cache::write(directory.as_deref(), cache);
}

/// True when we should hit the network now: no cache, stale cache (older than
/// the interval), or a cache timestamp in the future (clock moved backwards).
fn should_check(cache: &Option<UpdateCache>, now: DateTime<Utc>) -> bool {
    interval_elapsed(cache.as_ref().map(|c| c.last_checked_at), now)
}

/// True when `last` is missing, at least the interval old, or in the future (clock moved
/// backwards). Shared by the npm update check and the remote blocking-menu download.
fn interval_elapsed(last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match last {
        None => true,
        Some(last_checked_at) => {
            let age = now.signed_duration_since(last_checked_at);
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
pub async fn run_startup_check<H: UpdateCheckHost>(app: H, cache_state: Arc<OnceLock<UpdateInfo>>) {
    // 1. Read the settings toggle (default ON) once.
    let enabled = {
        let settings_state = app.update_settings();
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
        app.npm_update_available(info.clone());
        log::info!(
            "[update-check] update available: {} -> {}",
            info.current_version,
            info.latest_version
        );
    } else {
        log::debug!(
            "[update-check] up to date (current {}, latest {})",
            current,
            latest
        );
    }
}

/// GET the dist-tags and return `latest`. Fail-silent on offline/timeout
/// (returns None quietly). Logs at WARN (H1) when the registry *responds* but
/// the response is unusable (non-2xx, empty/oversized, or unparseable) so a
/// wrong URL or a future registry-shape change leaves a breadcrumb in app.log.
/// This warn-on-bad-response posture is the safety net for the no-test-gate
/// path: plain offline stays silent, a broken contract is loud.
async fn fetch_latest<H: UpdateCheckHost>(app: &H) -> Option<String> {
    let network = app.outbound_network();
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
    let enabled = crate::config::settings::load_settings_for_cli().npm_update_notifications_enabled;
    cli_notice(&cache, current, enabled)
}

// ---- Remote blocking-menu download (#1925) --------------------------------

/// What the remote blocking-menu download did, for the caller's log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteMenusCheck {
    Disabled,
    NotDue,
    Accepted,
    Rejected(String),
    Unreachable(String),
    WriteFailed(String),
}

/// One GET under the 10 s timeout. `Err` is a transport error or the timeout text; a
/// non-200 response keeps the status and an empty body. The body read stops as soon as it
/// passes the cap, so an oversized response is never fully buffered.
async fn fetch_remote_blocking_menus(
    network: &crate::network::OutboundNetwork,
    url: &str,
) -> Result<(u16, Vec<u8>), String> {
    let attempt = tokio::time::timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS), async {
        let mut response = network
            .general()
            .get(url)
            .header(
                reqwest::header::USER_AGENT,
                concat!("agentscommander/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        if status == 200 {
            while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
                body.extend_from_slice(&chunk);
                if body.len() > crate::config::settings::REMOTE_BLOCKING_MENUS_MAX_BYTES {
                    break;
                }
            }
        }
        Ok((status, body))
    })
    .await;
    match attempt {
        Err(_) => Err("timed out".to_string()),
        Ok(result) => result,
    }
}

/// #1925 (D3 to D5) - the download itself. The flag is checked first so a disabled run
/// touches no disk and no network; `NotDue` likewise. Every attempt that passes the due
/// check stamps `now` afterwards, accepted, rejected or unreachable, so the 24 h throttle
/// stays literal and a retry storm is impossible.
pub(crate) async fn run_remote_blocking_menus_check(
    network: &crate::network::OutboundNetwork,
    settings_path: &Path,
    enabled: bool,
    url: &str,
    now: DateTime<Utc>,
) -> RemoteMenusCheck {
    if !enabled {
        return RemoteMenusCheck::Disabled;
    }
    if !interval_elapsed(
        crate::config::settings::read_remote_blocking_menus_check_stamp(settings_path),
        now,
    ) {
        return RemoteMenusCheck::NotDue;
    }

    let outcome = match network.acquire("update_check.remote_blocking_menus").await {
        Err(error) => RemoteMenusCheck::Unreachable(error),
        Ok(_permit) => match fetch_remote_blocking_menus(network, url).await {
            Err(error) => RemoteMenusCheck::Unreachable(error),
            Ok((_, body))
                if body.len() > crate::config::settings::REMOTE_BLOCKING_MENUS_MAX_BYTES =>
            {
                RemoteMenusCheck::Rejected(format!(
                    "body larger than {} bytes",
                    crate::config::settings::REMOTE_BLOCKING_MENUS_MAX_BYTES
                ))
            }
            Ok((status, body)) => {
                match crate::config::settings::accept_remote_blocking_menus_response(status, &body)
                {
                    Err(reason) => RemoteMenusCheck::Rejected(reason),
                    Ok(file) => match crate::config::settings::write_remote_blocking_menus_cache(
                        settings_path,
                        file,
                        url,
                        now,
                    ) {
                        Ok(()) => RemoteMenusCheck::Accepted,
                        Err(error) => RemoteMenusCheck::WriteFailed(error),
                    },
                }
            }
        },
    };

    if let Err(error) =
        crate::config::settings::write_remote_blocking_menus_check_stamp(settings_path, now)
    {
        log::debug!("[remote-menus] could not write the throttle stamp: {error}");
    }
    outcome
}

/// #1925 (D3) - read the remote flag once, exactly as `run_startup_check` reads its own
/// flag, then hand the rest to `run_remote_blocking_menus_check`.
pub(crate) async fn run_remote_blocking_menus_for_app<H: UpdateCheckHost>(
    app: &H,
    settings_path: &Path,
    url: &str,
    now: DateTime<Utc>,
) -> RemoteMenusCheck {
    let enabled = {
        let settings_state = app.update_settings();
        let settings = settings_state.read().await;
        settings.remote_blocking_menus_enabled
    };
    let network = app.outbound_network();
    run_remote_blocking_menus_check(&network, settings_path, enabled, url, now).await
}

/// #1925 - the detached startup wrapper. Fail-silent: an accepted download logs one info
/// line, every other outcome logs at debug, and this never returns an error to the caller.
pub async fn run_remote_blocking_menus_startup<H: UpdateCheckHost>(app: H) {
    let Some(settings_path) = crate::config::settings::settings_path() else {
        log::debug!("[remote-menus] no settings path; skipping the remote download");
        return;
    };
    let outcome = run_remote_blocking_menus_for_app(
        &app,
        &settings_path,
        crate::config::settings::REMOTE_BLOCKING_MENUS_URL,
        Utc::now(),
    )
    .await;
    match outcome {
        RemoteMenusCheck::Accepted => log::info!(
            "[remote-menus] downloaded blocking-menu patterns; they apply at the next start"
        ),
        outcome => log::debug!("[remote-menus] {outcome:?}"),
    }
}

// ---- Remote per-agent help download (#2133) -------------------------------

/// What the remote agent-help download did, for the caller's log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteAgentHelpCheck {
    Disabled,
    NotDue,
    Accepted,
    Rejected(String),
    Unreachable(String),
    WriteFailed(String),
}

/// One GET under the 10 s timeout, as `fetch_remote_blocking_menus`. The body read stops as
/// soon as it passes the cap, so an oversized response is never fully buffered.
async fn fetch_remote_agent_help(
    network: &crate::network::OutboundNetwork,
    url: &str,
) -> Result<(u16, Vec<u8>), String> {
    let attempt = tokio::time::timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS), async {
        let mut response = network
            .general()
            .get(url)
            .header(
                reqwest::header::USER_AGENT,
                concat!("agentscommander/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        if status == 200 {
            while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
                body.extend_from_slice(&chunk);
                if body.len() > crate::config::settings::REMOTE_AGENT_HELP_MAX_BYTES {
                    break;
                }
            }
        }
        Ok((status, body))
    })
    .await;
    match attempt {
        Err(_) => Err("timed out".to_string()),
        Ok(result) => result,
    }
}

/// #2133 - the download itself, as `run_remote_blocking_menus_check`. The flag is checked
/// first so a disabled run touches no disk and no network; `NotDue` likewise. Every attempt
/// that passes the due check stamps `now` afterwards, accepted, rejected or unreachable, so
/// the 24 h throttle stays literal and a retry storm is impossible.
pub(crate) async fn run_remote_agent_help_check(
    network: &crate::network::OutboundNetwork,
    settings_path: &Path,
    enabled: bool,
    url: &str,
    now: DateTime<Utc>,
) -> RemoteAgentHelpCheck {
    if !enabled {
        return RemoteAgentHelpCheck::Disabled;
    }
    if !interval_elapsed(
        crate::config::settings::read_remote_agent_help_check_stamp(settings_path),
        now,
    ) {
        return RemoteAgentHelpCheck::NotDue;
    }

    let outcome = match network.acquire("update_check.remote_agent_help").await {
        Err(error) => RemoteAgentHelpCheck::Unreachable(error),
        Ok(_permit) => match fetch_remote_agent_help(network, url).await {
            Err(error) => RemoteAgentHelpCheck::Unreachable(error),
            Ok((status, body)) => {
                match crate::config::settings::accept_remote_agent_help_response(status, &body) {
                    Err(reason) => RemoteAgentHelpCheck::Rejected(reason),
                    Ok(file) => match crate::config::settings::write_remote_agent_help_cache(
                        settings_path,
                        file,
                        url,
                        now,
                    ) {
                        Ok(()) => RemoteAgentHelpCheck::Accepted,
                        Err(error) => RemoteAgentHelpCheck::WriteFailed(error),
                    },
                }
            }
        },
    };

    if let Err(error) =
        crate::config::settings::write_remote_agent_help_check_stamp(settings_path, now)
    {
        log::debug!("[agent-help] could not write the throttle stamp: {error}");
    }
    outcome
}

/// #2133 - the detached startup wrapper. Fail-silent: an accepted download logs one info line,
/// every other outcome logs at debug, and this never returns an error to the caller.
pub async fn run_remote_agent_help_startup<H: UpdateCheckHost>(app: H) {
    let Some(settings_path) = crate::config::settings::settings_path() else {
        log::debug!("[agent-help] no settings path; skipping the remote download");
        return;
    };
    let enabled = {
        let settings_state = app.update_settings();
        let settings = settings_state.read().await;
        settings.remote_agent_help_enabled
    };
    let network = app.outbound_network();
    let outcome = run_remote_agent_help_check(
        &network,
        &settings_path,
        enabled,
        crate::config::settings::REMOTE_AGENT_HELP_URL,
        Utc::now(),
    )
    .await;
    match outcome {
        RemoteAgentHelpCheck::Accepted => {
            log::info!("[agent-help] downloaded the remote help; it applies at the next start")
        }
        outcome => log::debug!("[agent-help] {outcome:?}"),
    }
}

#[cfg(test)]
mod tests;
