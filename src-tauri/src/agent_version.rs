//! #1551 - installed-version probe engine and session cache for coding-agent CLIs.
//!
//! Deliberately a SINK: it names no crate module except `pty::job`, so the
//! `commands::config`, `agent_update` and `web` SCC does not grow (plan #1551
//! section 11). Callers resolve programs and build rows; this module only runs
//! `<program> <fixed argv>` with a bound, parses, sanitizes, and caches.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Per-probe wall-clock cap. On expiry the whole tree is killed and the probe
/// reports `probeFailed`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// A committed install state is served without a process for this long.
pub const INSTALL_CACHE_TTL: Duration = Duration::from_secs(600);
/// Bytes retained per stream (the rest is drained and dropped).
pub const PROBE_OUTPUT_CAP: usize = 4 * 1024;
/// Sanitized diagnostic length, in characters.
pub const DETAIL_MAX_CHARS: usize = 160;
/// One cleanup attempt may spend this long proving the native tree empty. A
/// missed window records a defect but does not permit a terminal return; the
/// owned reaper starts another attempt and remains nonterminal until proof.
const ACTIVE_SETTLEMENT_WINDOW: Duration = Duration::from_secs(10);
const ACTIVE_SETTLEMENT_POLL: Duration = Duration::from_millis(25);

#[cfg(test)]
tokio::task_local! {
    static VERSION_PROBE_ARGS_OVERRIDE: (&'static str, &'static [&'static str]);
}

#[cfg(test)]
pub(crate) async fn with_version_probe_args_for_test<F>(
    stem: &'static str,
    args: &'static [&'static str],
    future: F,
) -> F::Output
where
    F: std::future::Future,
{
    VERSION_PROBE_ARGS_OVERRIDE
        .scope((stem, args), future)
        .await
}

/// #1551 - fixed probe argv per known program stem (lowercase, extension stripped).
/// No catalog/user/project string ever reaches argv. Cursor (`agent`) is absent on
/// purpose: it has no update command and its bare name collides with other vendors.
pub fn version_probe_args(program_stem: &str) -> Option<&'static [&'static str]> {
    #[cfg(test)]
    if let Ok((stem, args)) = VERSION_PROBE_ARGS_OVERRIDE.try_with(|override_| *override_) {
        if program_stem == stem {
            return Some(args);
        }
    }
    match program_stem {
        "claude" | "codex" | "hermes" | "pi" | "opencode" | "agy" => Some(&["--version"]),
        _ => None,
    }
}

/// The five install states a row can carry. `checking` is the client-visible
/// "no committed state yet" value, never a stored one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStatus {
    Checking,
    Missing,
    Installed,
    ProbeFailed,
    Unprobed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub status: InstallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// #1551 - commit counter of the install cache that stored this state; 0 until
    /// `ProbeTicket::complete` commits it (so `checking` and un-committed probe results
    /// are 0). Clients keep the highest `seq` per command (plan section 5.11).
    #[serde(default)]
    pub seq: u64,
}

impl InstallState {
    pub fn checking() -> Self {
        Self {
            status: InstallStatus::Checking,
            version: None,
            path: None,
            detail: None,
            seq: 0,
        }
    }

    pub fn missing(detail: String) -> Self {
        Self {
            status: InstallStatus::Missing,
            version: None,
            path: None,
            detail: Some(detail),
            seq: 0,
        }
    }

    pub fn installed(version: String, path: &Path) -> Self {
        Self {
            status: InstallStatus::Installed,
            version: Some(version),
            path: Some(path.display().to_string()),
            detail: None,
            seq: 0,
        }
    }

    pub fn probe_failed(path: &Path, detail: String) -> Self {
        Self {
            status: InstallStatus::ProbeFailed,
            version: None,
            path: Some(path.display().to_string()),
            detail: Some(detail),
            seq: 0,
        }
    }

    pub fn unprobed(path: &Path, detail: String) -> Self {
        Self {
            status: InstallStatus::Unprobed,
            version: None,
            path: Some(path.display().to_string()),
            detail: Some(detail),
            seq: 0,
        }
    }
}

/// #1551 - remove every ANSI CSI/OSC sequence, then every remaining control
/// character, then trim. A lone `ESC` or an unterminated sequence swallows the
/// rest of the line (there is no cross-line state: each line stands alone).
fn strip_ansi_and_controls(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            // CSI: parameter/intermediate bytes 0x20-0x3F, one final byte 0x40-0x7E.
            Some('[') => {
                let mut terminated = false;
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        terminated = true;
                        break;
                    }
                }
                if !terminated {
                    break;
                }
            }
            // OSC: everything up to and including BEL or the exact pair `ESC \`
            // (ST). An `ESC` followed by anything else is OSC content, so the
            // parser stays inside the sequence; it must not consume that next
            // character either, or it would eat the `ESC` of a later ST pair.
            Some(']') => {
                let mut terminated = false;
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        terminated = true;
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        terminated = true;
                        break;
                    }
                }
                if !terminated {
                    break;
                }
            }
            // A lone ESC (or ESC + anything else): drop the rest of the line.
            Some(_) | None => break,
        }
    }
    out.retain(|c| (c as u32) >= 0x20 && c != '\u{7f}');
    out.trim().to_string()
}

/// #1551 - every `\n` AND every `\r` is a line break (so `\r\n` yields an empty
/// line between), each piece sanitized, empty results skipped, order preserved.
pub fn text_lines(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(['\n', '\r'])
        .map(strip_ansi_and_controls)
        .filter(|line| !line.is_empty())
}

/// #1551 - the first line that carries visible text after sanitizing.
pub fn first_text_line(text: &str) -> Option<String> {
    text_lines(text).next()
}

/// #1551 - the version token of the FIRST text line only. A banner line before
/// the version is reported as `no version in output: <that line>` by the caller;
/// scanning further lines would trade that for false positives (plan section 10).
pub fn parse_version_token(text: &str) -> Option<String> {
    static VERSION_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = VERSION_RE
        .get_or_init(|| Regex::new(r"\bv?(\d+(?:\.\d+)+)").expect("version regex is valid"));
    let line = first_text_line(text)?;
    re.captures(&line)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

/// #1551 - a bounded, single-line, ANSI-free diagnostic. Callers substitute
/// `<empty>` / `<no output>` when it is empty.
pub fn sanitize_detail(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let line = first_text_line(&text).unwrap_or_default();
    if line.chars().count() > DETAIL_MAX_CHARS {
        let truncated: String = line.chars().take(DETAIL_MAX_CHARS).collect();
        format!("{truncated}...")
    } else {
        line
    }
}

/// Outcome of one probe process. `Failed` carries the sanitized detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    Version(String),
    Failed(String),
}

/// Outcome of a probe whose process owner observes a retained cancellation
/// signal. Cancelled and CleanupFailed are returned only after every native
/// process/handle and pipe reader is definitively settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancellableProbeOutcome {
    Completed(ProbeOutcome),
    Cancelled,
    CleanupFailed(String),
}

struct RecoverableProbeReader {
    completion: Shared<BoxFuture<'static, Result<Vec<u8>, String>>>,
    abort: tokio::task::AbortHandle,
    abort_requested: bool,
    #[cfg(test)]
    settlement_hook: Option<ProbeReaderSettlementHook>,
}

#[cfg(test)]
pub(crate) struct ProbeReaderSettlementHook {
    pub(crate) hold_completion: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) before_panic: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release_panic: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) retry_started: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Drain a pipe to EOF but retain only the first `PROBE_OUTPUT_CAP` bytes.
fn spawn_capped_reader<R>(
    mut reader: R,
    #[cfg(test)] mut settlement_hook: Option<ProbeReaderSettlementHook>,
) -> RecoverableProbeReader
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    #[cfg(test)]
    let hold_completion = settlement_hook
        .as_mut()
        .and_then(|hook| hook.hold_completion.take());
    let reader = tokio::spawn(async move {
        #[cfg(test)]
        if let Some(hold_completion) = hold_completion {
            let _ = hold_completion.await;
        }
        let mut kept: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match tokio::io::AsyncReadExt::read(&mut reader, &mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if kept.len() < PROBE_OUTPUT_CAP {
                        let room = PROBE_OUTPUT_CAP - kept.len();
                        kept.extend_from_slice(&chunk[..n.min(room)]);
                    }
                }
            }
        }
        kept
    });
    let abort = reader.abort_handle();
    let completion = async move { reader.await.map_err(|error| error.to_string()) }
        .boxed()
        .shared();
    RecoverableProbeReader {
        completion,
        abort,
        abort_requested: false,
        #[cfg(test)]
        settlement_hook,
    }
}

fn record_probe_attempt_expiry(defects: &mut Vec<String>, stage: &str) {
    let detail = format!("settlement attempt deadline exceeded during {stage}");
    if !defects.iter().any(|defect| defect == &detail) {
        defects.push(detail);
    }
}

async fn settle_probe_reader(
    reader: &mut Option<RecoverableProbeReader>,
    defects: &mut Vec<String>,
    attempt: &mut crate::pty::job::SettlementAttempt,
) -> Vec<u8> {
    let Some(reader) = reader.as_mut() else {
        return Vec::new();
    };
    let completion = reader.completion.clone();
    #[cfg(test)]
    if let Some(hook) = reader.settlement_hook.as_mut() {
        if let Some(release_panic) = hook.release_panic.take() {
            if let Some(before_panic) = hook.before_panic.take() {
                let _ = before_panic.send(());
            }
            let _ = release_panic.await;
            panic!("injected probe panic during reader settlement");
        }
        if let Some(retry_started) = hook.retry_started.take() {
            let _ = retry_started.send(());
        }
    }
    if reader.abort_requested {
        reader.abort.abort();
        let _ = completion.await;
        return Vec::new();
    }
    if attempt.expired() {
        record_probe_attempt_expiry(defects, "reader settlement");
        attempt.restart();
    }
    match tokio::time::timeout(attempt.remaining(), completion.clone()).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let detail = format!("reader join failed: {error}");
            if !defects.iter().any(|defect| defect == &detail) {
                defects.push(detail);
            }
            Vec::new()
        }
        Err(_) => {
            record_probe_attempt_expiry(defects, "reader settlement");
            reader.abort_requested = true;
            reader.abort.abort();
            let _ = completion.await;
            attempt.restart();
            Vec::new()
        }
    }
}

enum ProbeWait {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

struct ProbeSettlement {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    defects: Vec<String>,
}

#[cfg(test)]
pub(crate) struct ProbeSettlementHook {
    pub(crate) before_native_proof: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release_native_proof: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) settlement_window: Option<Duration>,
    pub(crate) native_observations: std::collections::VecDeque<ProbeNativeObservation>,
    pub(crate) panic_after_native_proof_release: bool,
    pub(crate) panic_after_native_query_defect: bool,
    pub(crate) stdout_reader: Option<ProbeReaderSettlementHook>,
}

#[cfg(test)]
pub(crate) enum ProbeNativeObservation {
    Empty,
    Active,
    Error(&'static str),
}

#[cfg(test)]
struct ProbeSettlementHookScope {
    remaining_spawns: usize,
    hook: Option<ProbeSettlementHook>,
}

#[cfg(test)]
tokio::task_local! {
    static PROBE_SETTLEMENT_HOOK: std::cell::RefCell<ProbeSettlementHookScope>;
}

#[cfg(test)]
pub(crate) async fn with_probe_settlement_hook<F>(hook: ProbeSettlementHook, future: F) -> F::Output
where
    F: std::future::Future,
{
    PROBE_SETTLEMENT_HOOK
        .scope(
            std::cell::RefCell::new(ProbeSettlementHookScope {
                remaining_spawns: 0,
                hook: Some(hook),
            }),
            future,
        )
        .await
}

#[cfg(test)]
pub(crate) async fn with_probe_settlement_hook_after_spawns<F>(
    remaining_spawns: usize,
    hook: ProbeSettlementHook,
    future: F,
) -> F::Output
where
    F: std::future::Future,
{
    PROBE_SETTLEMENT_HOOK
        .scope(
            std::cell::RefCell::new(ProbeSettlementHookScope {
                remaining_spawns,
                hook: Some(hook),
            }),
            future,
        )
        .await
}

struct ProbeProcessOwner {
    child: Option<tokio::process::Child>,
    stdout: Option<RecoverableProbeReader>,
    stderr: Option<RecoverableProbeReader>,
    defects: Vec<String>,
    #[cfg(windows)]
    job: Option<crate::pty::job::JobObject>,
    #[cfg(unix)]
    pgid: i32,
    #[cfg(test)]
    settlement_hook: Option<ProbeSettlementHook>,
}

impl ProbeProcessOwner {
    async fn spawn(command: &mut tokio::process::Command) -> Result<Self, CancellableProbeOutcome> {
        #[cfg(windows)]
        let (mut child, job) = match crate::pty::job::spawn_suspended_contained(command).await {
            Ok(pair) => pair,
            Err(crate::pty::job::ContainedSpawnError::Spawn(error)) => {
                return Err(CancellableProbeOutcome::Completed(ProbeOutcome::Failed(
                    format!("spawn failed: {error}"),
                )))
            }
            Err(crate::pty::job::ContainedSpawnError::Containment(reason)) => {
                log::warn!(
                    "[agent-version] contained probe launch rejected: process containment failed at {reason}"
                );
                return Err(CancellableProbeOutcome::Completed(ProbeOutcome::Failed(
                    "Version probe process-tree containment unavailable.".to_string(),
                )));
            }
            Err(crate::pty::job::ContainedSpawnError::Cleanup(detail)) => {
                log::warn!("[agent-version] contained probe launch cleanup defective: {detail}");
                return Err(CancellableProbeOutcome::CleanupFailed(detail));
            }
        };

        #[cfg(not(windows))]
        let mut child = command.spawn().map_err(|error| {
            CancellableProbeOutcome::Completed(ProbeOutcome::Failed(format!(
                "spawn failed: {error}"
            )))
        })?;

        #[cfg(unix)]
        let pgid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .unwrap_or(0);
        #[cfg(test)]
        let mut settlement_hook = PROBE_SETTLEMENT_HOOK
            .try_with(|scope| {
                let mut scope = scope.borrow_mut();
                if scope.remaining_spawns == 0 {
                    scope.hook.take()
                } else {
                    scope.remaining_spawns -= 1;
                    None
                }
            })
            .ok()
            .flatten();
        #[cfg(test)]
        let stdout_reader_hook = settlement_hook
            .as_mut()
            .and_then(|hook| hook.stdout_reader.take());
        #[cfg(test)]
        let stdout = child
            .stdout
            .take()
            .map(move |reader| spawn_capped_reader(reader, stdout_reader_hook));
        #[cfg(test)]
        let stderr = child
            .stderr
            .take()
            .map(|reader| spawn_capped_reader(reader, None));
        #[cfg(not(test))]
        let stdout = child.stdout.take().map(spawn_capped_reader);
        #[cfg(not(test))]
        let stderr = child.stderr.take().map(spawn_capped_reader);
        Ok(Self {
            child: Some(child),
            stdout,
            stderr,
            defects: Vec::new(),
            #[cfg(windows)]
            job: Some(job),
            #[cfg(unix)]
            pgid,
            #[cfg(test)]
            settlement_hook,
        })
    }

    async fn wait(
        &mut self,
        timeout: Duration,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
    ) -> ProbeWait {
        let child = self.child.as_mut().expect("probe child owner");
        tokio::select! {
            result = child.wait() => ProbeWait::Exited(result),
            _ = tokio::time::sleep(timeout) => ProbeWait::TimedOut,
            _ = wait_for_probe_cancellation(cancel) => ProbeWait::Cancelled,
        }
    }

    async fn settle(&mut self, wait: &ProbeWait) -> ProbeSettlement {
        #[cfg(test)]
        let settlement_window = self
            .settlement_hook
            .as_ref()
            .and_then(|hook| hook.settlement_window)
            .unwrap_or(ACTIVE_SETTLEMENT_WINDOW);
        #[cfg(not(test))]
        let settlement_window = ACTIVE_SETTLEMENT_WINDOW;
        let mut attempt = crate::pty::job::SettlementAttempt::new(settlement_window);
        let defects = &mut self.defects;
        let cleanup_required = !matches!(wait, ProbeWait::Exited(Ok(_)));

        #[cfg(windows)]
        if cleanup_required {
            if let Some(job) = self.job.as_ref() {
                if let Err(error) = job.terminate_checked() {
                    defects.push(format!("job termination failed: {error}"));
                }
            }
        }

        #[cfg(unix)]
        if cleanup_required && self.pgid > 0 {
            // SAFETY: this is the positive process-group id created by
            // `Command::process_group(0)` for this probe owner.
            let killed = unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
            if killed != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                defects.push(format!(
                    "process-group termination failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }

        if cleanup_required {
            if let Some(child) = self.child.as_mut() {
                if let Err(error) = child.start_kill() {
                    defects.push(format!("direct child termination failed: {error}"));
                }
            }
        }

        if cleanup_required {
            if let Some(child) = self.child.as_mut() {
                let mut wait_defect_recorded = false;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {}
                        Err(error) => {
                            if !wait_defect_recorded {
                                defects.push(format!("direct child wait failed: {error}"));
                                wait_defect_recorded = true;
                            }
                        }
                    }
                    if attempt.expired() {
                        record_probe_attempt_expiry(defects, "direct-child settlement");
                        #[cfg(windows)]
                        if let Some(job) = self.job.as_ref() {
                            let _ = job.terminate_checked();
                        }
                        #[cfg(unix)]
                        if self.pgid > 0 {
                            // SAFETY: this remains the retained group identity.
                            unsafe {
                                libc::kill(-self.pgid, libc::SIGKILL);
                            }
                        }
                        let _ = child.start_kill();
                        attempt.restart();
                    }
                    tokio::time::sleep(ACTIVE_SETTLEMENT_POLL.min(attempt.remaining())).await;
                }
            }
        }

        // Release Tokio's direct process handle before the first native
        // emptiness query that is eligible to prove settlement.
        drop(self.child.take());

        #[cfg(windows)]
        if !cleanup_required {
            if let Some(job) = self.job.as_ref() {
                job.terminate();
            }
        }
        #[cfg(unix)]
        if !cleanup_required && self.pgid > 0 {
            // SAFETY: same owned positive group identity as above.
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
        }

        let stdout = settle_probe_reader(&mut self.stdout, defects, &mut attempt).await;
        let stderr = settle_probe_reader(&mut self.stderr, defects, &mut attempt).await;

        #[cfg(test)]
        {
            let (ready, mut release) = self
                .settlement_hook
                .as_mut()
                .map(|hook| {
                    (
                        hook.before_native_proof.take(),
                        hook.release_native_proof.take(),
                    )
                })
                .unwrap_or((None, None));
            if let Some(ready) = ready {
                let _ = ready.send(());
            }
            if let Some(release) = release.as_mut() {
                loop {
                    if attempt.expired() {
                        record_probe_attempt_expiry(defects, "native-proof barrier");
                        attempt.restart();
                    }
                    match tokio::time::timeout(attempt.remaining(), &mut *release).await {
                        Ok(_) => break,
                        Err(_) => {
                            record_probe_attempt_expiry(defects, "native-proof barrier");
                            attempt.restart();
                        }
                    }
                }
            }
            let panic_after_release = self
                .settlement_hook
                .as_mut()
                .is_some_and(|hook| std::mem::take(&mut hook.panic_after_native_proof_release));
            if panic_after_release {
                panic!("injected probe panic during native settlement");
            }
        }

        #[cfg(windows)]
        if let Some(job) = self.job.as_ref() {
            let mut query_defect_recorded = false;
            loop {
                #[cfg(test)]
                let proof = match self
                    .settlement_hook
                    .as_mut()
                    .and_then(|hook| hook.native_observations.pop_front())
                {
                    Some(observation) => match observation {
                        ProbeNativeObservation::Empty => Ok(true),
                        ProbeNativeObservation::Active => Ok(false),
                        ProbeNativeObservation::Error(detail) => Err(detail.to_string()),
                    },
                    None => job
                        .active_processes()
                        .map(|active| active == 0)
                        .map_err(|error| error.to_string()),
                };
                #[cfg(not(test))]
                let proof = job
                    .active_processes()
                    .map(|active| active == 0)
                    .map_err(|error| error.to_string());
                match proof {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => {
                        if !query_defect_recorded {
                            defects.push(format!("job accounting failed: {error}"));
                            query_defect_recorded = true;
                        }
                        #[cfg(test)]
                        if self.settlement_hook.as_mut().is_some_and(|hook| {
                            std::mem::take(&mut hook.panic_after_native_query_defect)
                        }) {
                            panic!("injected probe panic after native query defect");
                        }
                    }
                }
                if attempt.expired() {
                    record_probe_attempt_expiry(defects, "job accounting proof");
                    let _ = job.terminate_checked();
                    attempt.restart();
                }
                tokio::time::sleep(ACTIVE_SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }

        #[cfg(unix)]
        if self.pgid > 0 {
            let mut errno_defect_recorded = false;
            loop {
                #[cfg(test)]
                let proof = match self
                    .settlement_hook
                    .as_mut()
                    .and_then(|hook| hook.native_observations.pop_front())
                {
                    Some(observation) => match observation {
                        ProbeNativeObservation::Empty => Ok(true),
                        ProbeNativeObservation::Active => Ok(false),
                        ProbeNativeObservation::Error(detail) => Err(detail.to_string()),
                    },
                    None => {
                        // SAFETY: signal zero performs no mutation and checks the retained PGID.
                        let result = unsafe { libc::kill(-self.pgid, 0) };
                        if result == -1 {
                            let error = std::io::Error::last_os_error();
                            if error.raw_os_error() == Some(libc::ESRCH) {
                                Ok(true)
                            } else {
                                Err(error.to_string())
                            }
                        } else {
                            Ok(false)
                        }
                    }
                };
                #[cfg(not(test))]
                let proof = {
                    // SAFETY: signal zero performs no mutation and checks the retained PGID.
                    let result = unsafe { libc::kill(-self.pgid, 0) };
                    if result == -1 {
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() == Some(libc::ESRCH) {
                            Ok(true)
                        } else {
                            Err(error.to_string())
                        }
                    } else {
                        Ok(false)
                    }
                };
                match proof {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => {
                        if !errno_defect_recorded {
                            defects.push(format!("process-group accounting failed: {error}"));
                            errno_defect_recorded = true;
                        }
                        #[cfg(test)]
                        if self.settlement_hook.as_mut().is_some_and(|hook| {
                            std::mem::take(&mut hook.panic_after_native_query_defect)
                        }) {
                            panic!("injected probe panic after native query defect");
                        }
                    }
                }
                if attempt.expired() {
                    record_probe_attempt_expiry(defects, "process-group accounting proof");
                    // SAFETY: this remains the retained positive process group.
                    unsafe {
                        libc::kill(-self.pgid, libc::SIGKILL);
                    }
                    attempt.restart();
                }
                tokio::time::sleep(ACTIVE_SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }

        #[cfg(windows)]
        drop(self.job.take());
        ProbeSettlement {
            stdout,
            stderr,
            defects: defects.clone(),
        }
    }

    async fn settle_observing_cancellation<F>(
        &mut self,
        wait: &ProbeWait,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
        observed: F,
    ) -> (ProbeSettlement, bool)
    where
        F: FnOnce(),
    {
        if matches!(wait, ProbeWait::Cancelled) {
            observed();
            return (self.settle(wait).await, true);
        }
        let settlement = self.settle(wait);
        tokio::pin!(settlement);
        tokio::select! {
            biased;
            _ = wait_for_probe_cancellation(cancel) => {
                observed();
                (settlement.await, true)
            }
            settled = &mut settlement => (settled, false),
        }
    }
}

/// The startup-update target keeps this slot outside its caught future. The
/// private process owner therefore survives a panic while wait/settlement is
/// being polled and can resume cleanup before any terminal publication.
#[derive(Default)]
pub(crate) struct RetainedProbeOwner {
    owner: Option<ProbeProcessOwner>,
}

impl RetainedProbeOwner {
    pub(crate) fn is_some(&self) -> bool {
        self.owner.is_some()
    }

    pub(crate) fn clear(&mut self) {
        drop(self.owner.take());
    }

    pub(crate) async fn settle_after_panic<F>(
        &mut self,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
        observed: F,
    ) -> Option<String>
    where
        F: FnOnce(),
    {
        let owner = self.owner.as_mut()?;
        let (settlement, _) = owner
            .settle_observing_cancellation(&ProbeWait::Cancelled, cancel, observed)
            .await;
        (!settlement.defects.is_empty()).then(|| settlement.defects.join("; "))
    }
}

async fn wait_for_probe_cancellation(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn parse_probe_completion(
    program: &Path,
    status: std::process::ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> ProbeOutcome {
    if status.success() {
        match parse_version_token(&String::from_utf8_lossy(stdout))
            .or_else(|| parse_version_token(&String::from_utf8_lossy(stderr)))
        {
            Some(version) => {
                log::info!(
                    "[agent-version] {} -> installed version {}",
                    program.display(),
                    version
                );
                ProbeOutcome::Version(version)
            }
            None => {
                let mut detail = sanitize_detail(stdout);
                if detail.is_empty() {
                    detail = "<empty>".to_string();
                }
                let detail = format!("no version in output: {detail}");
                log::warn!(
                    "[agent-version] probe failed for {}: {detail}",
                    program.display()
                );
                ProbeOutcome::Failed(detail)
            }
        }
    } else {
        let mut tail = sanitize_detail(stderr);
        if tail.is_empty() {
            tail = sanitize_detail(stdout);
        }
        if tail.is_empty() {
            tail = "<no output>".to_string();
        }
        let detail = format!("exit code {}: {}", status.code().unwrap_or(-1), tail);
        log::warn!(
            "[agent-version] probe failed for {}: {detail}",
            program.display()
        );
        ProbeOutcome::Failed(detail)
    }
}

/// #1551 - run `<program> <args>` with a bound and read its version.
///
/// argv only, never a shell string; stdin closed; no console window on Windows;
/// its own process group on Unix; the whole tree dies on timeout. The update
/// runner is NOT reused: it executes shell strings, is pinned by six tests, and
/// has a different failure contract.
pub async fn probe_version(program: &Path, args: &[&str], timeout: Duration) -> ProbeOutcome {
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    match probe_version_cancellable(program, args, timeout, cancel_rx).await {
        CancellableProbeOutcome::Completed(outcome) => outcome,
        CancellableProbeOutcome::Cancelled => {
            ProbeOutcome::Failed("probe cancelled unexpectedly".to_string())
        }
        CancellableProbeOutcome::CleanupFailed(detail) => ProbeOutcome::Failed(detail),
    }
}

pub async fn probe_version_cancellable(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> CancellableProbeOutcome {
    let mut owner = RetainedProbeOwner::default();
    let outcome = probe_version_cancellable_retained(
        program,
        args,
        timeout,
        &mut cancel,
        &mut owner,
        || {},
        || {},
    )
    .await;
    owner.clear();
    outcome
}

/// Startup-update adapter with externally retained ownership. `owner_installed`
/// runs synchronously only after the real process owner is stored in `owner_slot`;
/// `cancellation_observed` runs while that owner is still retained and before
/// settlement is awaited to completion.
pub(crate) async fn probe_version_cancellable_retained<I, O>(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    owner_slot: &mut RetainedProbeOwner,
    owner_installed: I,
    cancellation_observed: O,
) -> CancellableProbeOutcome
where
    I: FnOnce(),
    O: FnOnce(),
{
    let mut command = {
        let mut c = tokio::process::Command::new(program);
        c.args(args);
        c.stdin(Stdio::null());
        c.stdout(Stdio::piped());
        c.stderr(Stdio::piped());
        c.kill_on_drop(true);
        c
    };
    #[cfg(unix)]
    {
        command.process_group(0);
    }

    let owner = match ProbeProcessOwner::spawn(&mut command).await {
        Ok(owner) => owner,
        Err(outcome) => return outcome,
    };
    debug_assert!(!owner_slot.is_some());
    owner_slot.owner = Some(owner);
    owner_installed();
    let owner = owner_slot.owner.as_mut().expect("retained probe owner");
    let waited = owner.wait(timeout, cancel).await;
    let (settlement, cancellation_was_observed) = owner
        .settle_observing_cancellation(&waited, cancel, cancellation_observed)
        .await;
    if !settlement.defects.is_empty() {
        return CancellableProbeOutcome::CleanupFailed(settlement.defects.join("; "));
    }
    if cancellation_was_observed {
        return CancellableProbeOutcome::Cancelled;
    }
    match &waited {
        ProbeWait::Cancelled => CancellableProbeOutcome::Cancelled,
        ProbeWait::TimedOut => {
            let detail = format!("timed out after {}s (killed)", timeout.as_secs());
            log::warn!(
                "[agent-version] probe failed for {}: {detail}",
                program.display()
            );
            CancellableProbeOutcome::Completed(ProbeOutcome::Failed(detail))
        }
        ProbeWait::Exited(Err(error)) => {
            let detail = format!("spawn failed: {error}");
            log::warn!(
                "[agent-version] probe failed for {}: {detail}",
                program.display()
            );
            CancellableProbeOutcome::Completed(ProbeOutcome::Failed(detail))
        }
        ProbeWait::Exited(Ok(status)) => CancellableProbeOutcome::Completed(
            parse_probe_completion(program, *status, &settlement.stdout, &settlement.stderr),
        ),
    }
}

/// #1551 - process-lifetime install-state cache, keyed by the catalog `command`
/// string (the update unit, like `agentAutoUpdateByCommand`).
#[derive(Default)]
pub struct AgentInstallCache {
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    generation: u64,
    seq: u64,
    entries: HashMap<String, CachedInstall>,
    in_flight: HashSet<String>,
}

struct CachedInstall {
    probed_at: Instant,
    state: InstallState,
}

/// Read-only view of one command's cache slot.
#[derive(Clone, Debug)]
pub enum CacheLookup {
    Fresh(InstallState),
    InFlight,
    Absent,
}

/// #1551 - result of the single-lock scheduling operation.
pub enum Scheduling {
    Fresh(InstallState),
    InFlight,
    Began(ProbeTicket),
    Deferred,
}

/// The exclusive right to probe one command and commit its state.
pub struct ProbeTicket {
    cache: Arc<AgentInstallCache>,
    command: String,
    generation: u64,
    live: bool,
}

/// #1551 - result of closing a ticket: the state is committed to the current generation,
/// or the cache was invalidated meanwhile and the SAME ticket continues (renewed) for a retry.
pub enum Completion {
    Committed(InstallState),
    Stale(ProbeTicket),
}

impl AgentInstallCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, CacheInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Fresh iff an entry exists and `now.duration_since(probed_at) <= ttl` (the
    /// stored state, seq included); else InFlight iff a ticket is open for
    /// `command`; else Absent (an expired entry is removed on the way).
    pub fn lookup(&self, command: &str, now: Instant, ttl: Duration) -> CacheLookup {
        let mut inner = self.lock();
        let expired = match inner.entries.get(command) {
            Some(entry) if now.duration_since(entry.probed_at) <= ttl => {
                return CacheLookup::Fresh(entry.state.clone())
            }
            Some(_) => true,
            None => false,
        };
        if expired {
            inner.entries.remove(command);
        }
        if inner.in_flight.contains(command) {
            return CacheLookup::InFlight;
        }
        CacheLookup::Absent
    }

    /// #1551 - the ONLY production scheduling primitive: ONE critical section decides between serving a fresh
    /// entry, reporting an open ticket, opening the ticket (iff `may_begin`; `in_flight` insert + generation
    /// capture in the same lock take), or deferring. Two concurrent callers can never both receive `Began` for
    /// the same command, and a commit that lands before this call is always served as `Fresh`: there is no
    /// observable state between "looked up" and "began".
    pub fn lookup_or_begin(
        self: &Arc<Self>,
        command: &str,
        now: Instant,
        ttl: Duration,
        may_begin: bool,
    ) -> Scheduling {
        let mut inner = self.lock();
        let expired = match inner.entries.get(command) {
            Some(entry) if now.duration_since(entry.probed_at) <= ttl => {
                return Scheduling::Fresh(entry.state.clone())
            }
            Some(_) => true,
            None => false,
        };
        if expired {
            inner.entries.remove(command);
        }
        if inner.in_flight.contains(command) {
            return Scheduling::InFlight;
        }
        if !may_begin {
            return Scheduling::Deferred;
        }
        inner.in_flight.insert(command.to_string());
        let generation = inner.generation;
        drop(inner);
        Scheduling::Began(ProbeTicket {
            cache: Arc::clone(self),
            command: command.to_string(),
            generation,
            live: true,
        })
    }

    /// #1551 - after a startup pass every entry is stale: `generation += 1`, `entries.clear()`. Tickets stay open
    /// (their next `complete` returns `Stale`) and `seq` is NOT reset, so every post-invalidation commit outranks
    /// every pre-invalidation state on the clients.
    pub fn invalidate_all(&self) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.entries.clear();
    }

    /// Test seeding only (opens a ticket without consulting entries); production never schedules through it.
    #[cfg(test)]
    pub fn try_begin(self: &Arc<Self>, command: &str) -> Option<ProbeTicket> {
        let mut inner = self.lock();
        if !inner.in_flight.insert(command.to_string()) {
            return None;
        }
        let generation = inner.generation;
        drop(inner);
        Some(ProbeTicket {
            cache: Arc::clone(self),
            command: command.to_string(),
            generation,
            live: true,
        })
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    #[cfg(test)]
    pub fn seq(&self) -> u64 {
        self.lock().seq
    }

    #[cfg(test)]
    pub fn in_flight_len(&self) -> usize {
        self.lock().in_flight.len()
    }
}

impl ProbeTicket {
    /// Under the cache lock: if `generation` still matches, `seq += 1`, stamp `state.seq`, insert the entry
    /// (`probed_at = now`), remove `command` from `in_flight`, set `live = false`, return `Committed(stamped)`.
    /// Otherwise adopt the current generation and return `Stale(self)`: the ticket is NEVER released between
    /// the rejection and the retry, so no competing `lookup_or_begin` can take the slot in between.
    pub fn complete(mut self, state: InstallState) -> Completion {
        // The guard is confined to this block so `self` is only ever dropped
        // after the cache lock was released.
        let committed = {
            let mut inner = self.cache.lock();
            if inner.generation == self.generation {
                inner.seq += 1;
                let mut stamped = state;
                stamped.seq = inner.seq;
                inner.entries.insert(
                    self.command.clone(),
                    CachedInstall {
                        probed_at: Instant::now(),
                        state: stamped.clone(),
                    },
                );
                inner.in_flight.remove(&self.command);
                self.live = false;
                Some(stamped)
            } else {
                self.generation = inner.generation;
                None
            }
        };
        match committed {
            Some(stamped) => Completion::Committed(stamped),
            None => Completion::Stale(self),
        }
    }
}

impl Drop for ProbeTicket {
    /// Removes `command` from `in_flight` ONLY while `live` (an un-completed ticket: task panicked, aborted,
    /// or a second `Stale` was dropped). A committed ticket already cleared `live`, so its drop can never free
    /// the slot of a later ticket for the same command.
    fn drop(&mut self) {
        if self.live {
            let mut inner = self.cache.lock();
            inner.in_flight.remove(&self.command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn shell_program() -> &'static str {
        if cfg!(windows) {
            "cmd.exe"
        } else {
            "sh"
        }
    }

    #[test]
    fn parse_version_token_fixtures() {
        assert_eq!(
            parse_version_token("2.1.245 (Claude Code)").as_deref(),
            Some("2.1.245")
        );
        assert_eq!(
            parse_version_token("codex-cli 0.149.1").as_deref(),
            Some("0.149.1")
        );
        assert_eq!(parse_version_token("0.84.3").as_deref(), Some("0.84.3"));
        assert_eq!(
            parse_version_token("Hermes Agent v0.17.0 (2026.6.19) \u{b7} upstream").as_deref(),
            Some("0.17.0")
        );
        assert_eq!(parse_version_token("1.1.20").as_deref(), Some("1.1.20"));
        assert_eq!(
            parse_version_token("\n\n  1.18.23\n").as_deref(),
            Some("1.18.23")
        );
        assert_eq!(
            parse_version_token("\r\n\r\n1.2.3\r\n").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            parse_version_token("\n\x1b[?25l\n2.0.1\n").as_deref(),
            Some("2.0.1")
        );
        assert_eq!(
            parse_version_token("\x1b[32mv1.2.3\x1b[0m\nsecond").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(parse_version_token("hello\n1.2.3"), None);
        assert_eq!(parse_version_token("hello world"), None);
        assert_eq!(parse_version_token(""), None);
        assert_eq!(parse_version_token("\x1b[?25l\n"), None);
    }

    #[test]
    fn first_text_line_skips_blank_and_ansi_only_lines() {
        assert_eq!(
            first_text_line("\n\x1b[?25l\n2.0.1\n").as_deref(),
            Some("2.0.1")
        );
        assert_eq!(first_text_line("\r\n  x  \r\n").as_deref(), Some("x"));
        assert_eq!(first_text_line("\x1b[?25l\n"), None);
        assert_eq!(first_text_line(""), None);
    }

    /// #1551 - an OSC sequence ends ONLY at BEL or at the exact pair `ESC \` (ST).
    /// An `ESC` followed by anything else is OSC content: the parser stays inside
    /// the sequence and keeps discarding, so malformed or adversarial CLI output
    /// cannot leak sequence bytes into the version token or the diagnostic.
    #[test]
    fn osc_ends_only_at_bel_or_st() {
        // ESC ] foo ESC X bar BEL 1.2.3 -> the whole OSC is dropped.
        let esc_not_st = "\x1b]foo\x1bXbar\x071.2.3";
        assert_eq!(strip_ansi_and_controls(esc_not_st), "1.2.3");
        assert_eq!(first_text_line(esc_not_st).as_deref(), Some("1.2.3"));
        assert_eq!(parse_version_token(esc_not_st).as_deref(), Some("1.2.3"));
        assert_eq!(sanitize_detail(esc_not_st.as_bytes()), "1.2.3");

        // ST terminates it, and the text after ST survives.
        let st = "\x1b]0;window title\x1b\\2.0.1";
        assert_eq!(strip_ansi_and_controls(st), "2.0.1");
        assert_eq!(parse_version_token(st).as_deref(), Some("2.0.1"));

        // A non-ST ESC inside OSC never consumes the ESC of a later ST pair.
        assert_eq!(strip_ansi_and_controls("\x1b]a\x1b\x1b\\3.1.4"), "3.1.4");

        // BEL still terminates, as before.
        assert_eq!(strip_ansi_and_controls("\x1b]0;title\x074.5.6"), "4.5.6");

        // An OSC that never terminates still swallows the rest of the line.
        let unterminated = "\x1b]never ends 9.9.9";
        assert_eq!(strip_ansi_and_controls(unterminated), "");
        assert_eq!(first_text_line(unterminated), None);
        assert_eq!(parse_version_token(unterminated), None);
        assert_eq!(sanitize_detail(unterminated.as_bytes()), "");
    }

    #[test]
    fn sanitize_detail_strips_ansi_controls_and_truncates() {
        assert_eq!(sanitize_detail(b"\x1b[32mv1.2.3\x1b[0m\nsecond"), "v1.2.3");
        assert_eq!(sanitize_detail(b"\n\x1b[?25l\n2.0.1\n"), "2.0.1");
        let long = "a".repeat(400);
        let truncated = sanitize_detail(long.as_bytes());
        assert_eq!(truncated.chars().count(), DETAIL_MAX_CHARS + 3);
        assert!(truncated.ends_with("..."));
        assert_eq!(&truncated[..DETAIL_MAX_CHARS], "a".repeat(DETAIL_MAX_CHARS));
        assert_eq!(sanitize_detail(b""), "");
    }

    #[tokio::test]
    async fn probe_version_parses_echoed_version() {
        let args: Vec<&str> = if cfg!(windows) {
            vec!["/C", "echo 1.2.3"]
        } else {
            vec!["-c", "echo 1.2.3"]
        };
        let outcome = probe_version(Path::new(shell_program()), &args, PROBE_TIMEOUT).await;
        assert_eq!(outcome, ProbeOutcome::Version("1.2.3".to_string()));
    }

    #[tokio::test]
    async fn probe_version_skips_leading_blank_lines() {
        let args: Vec<&str> = if cfg!(windows) {
            vec!["/C", "echo.& echo 1.2.3"]
        } else {
            vec!["-c", "printf '\\n\\n1.2.3\\n'"]
        };
        let outcome = probe_version(Path::new(shell_program()), &args, PROBE_TIMEOUT).await;
        assert_eq!(outcome, ProbeOutcome::Version("1.2.3".to_string()));
    }

    #[tokio::test]
    async fn probe_version_skips_an_ansi_only_first_line() {
        let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
            (
                "powershell.exe",
                vec![
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[char]27 + '[?25l'; '2.0.1'",
                ],
            )
        } else {
            ("sh", vec!["-c", "printf '\\033[?25l\\n2.0.1\\n'"])
        };
        let outcome = probe_version(Path::new(program), &args, PROBE_TIMEOUT).await;
        assert_eq!(outcome, ProbeOutcome::Version("2.0.1".to_string()));
    }

    #[tokio::test]
    async fn probe_version_nonzero_exit_fails_with_code() {
        let args: Vec<&str> = if cfg!(windows) {
            vec!["/C", "exit 3"]
        } else {
            vec!["-c", "exit 3"]
        };
        let outcome = probe_version(Path::new(shell_program()), &args, PROBE_TIMEOUT).await;
        match outcome {
            ProbeOutcome::Failed(detail) => {
                assert!(detail.starts_with("exit code 3"), "unexpected: {detail}")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_version_no_token_fails() {
        let args: Vec<&str> = if cfg!(windows) {
            vec!["/C", "echo hello"]
        } else {
            vec!["-c", "echo hello"]
        };
        let outcome = probe_version(Path::new(shell_program()), &args, PROBE_TIMEOUT).await;
        match outcome {
            ProbeOutcome::Failed(detail) => assert!(
                detail.starts_with("no version in output: hello"),
                "unexpected: {detail}"
            ),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_version_timeout_kills_tree() {
        let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
            ("cmd.exe", vec!["/C", "ping", "-n", "30", "127.0.0.1"])
        } else {
            ("sh", vec!["-c", "sleep 30"])
        };
        let started = Instant::now();
        let outcome = probe_version(Path::new(program), &args, Duration::from_millis(200)).await;
        match outcome {
            ProbeOutcome::Failed(detail) => {
                assert!(detail.contains("timed out"), "unexpected: {detail}")
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "probe timeout must kill the tree promptly"
        );
    }

    async fn cancel_blocked_probe(program: &str, args: Vec<&str>) {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let probe = tokio::spawn({
            let program = PathBuf::from(program);
            let args: Vec<String> = args.into_iter().map(str::to_string).collect();
            async move {
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                probe_version_cancellable(&program, &borrowed, Duration::from_secs(30), cancel_rx)
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel_tx.send(true).expect("request cancellation");
        let outcome = tokio::time::timeout(Duration::from_secs(15), probe)
            .await
            .expect("probe cancellation settled")
            .expect("probe task");
        assert_eq!(outcome, CancellableProbeOutcome::Cancelled);
    }

    #[cfg(any(windows, unix))]
    async fn cancel_during_probe_settlement(
        program: &str,
        args: Vec<&str>,
        native_observations: std::collections::VecDeque<ProbeNativeObservation>,
    ) -> CancellableProbeOutcome {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let (proof_ready_tx, proof_ready_rx) = tokio::sync::oneshot::channel();
        let (release_proof_tx, release_proof_rx) = tokio::sync::oneshot::channel();
        let probe = tokio::spawn({
            let program = PathBuf::from(program);
            let args: Vec<String> = args.into_iter().map(str::to_string).collect();
            async move {
                with_probe_settlement_hook(
                    ProbeSettlementHook {
                        before_native_proof: Some(proof_ready_tx),
                        release_native_proof: Some(release_proof_rx),
                        settlement_window: Some(Duration::from_millis(60)),
                        native_observations,
                        panic_after_native_proof_release: false,
                        panic_after_native_query_defect: false,
                        stdout_reader: None,
                    },
                    async move {
                        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                        probe_version_cancellable(
                            &program,
                            &borrowed,
                            Duration::from_secs(10),
                            cancel_rx,
                        )
                        .await
                    },
                )
                .await
            }
        });
        proof_ready_rx
            .await
            .expect("probe direct handle dropped before native proof");
        cancel_tx
            .send(true)
            .expect("request cancellation during settlement");
        tokio::task::yield_now().await;
        assert!(
            !probe.is_finished(),
            "probe terminal outcome must remain behind native proof"
        );
        release_proof_tx
            .send(())
            .expect("release probe native proof");
        tokio::time::timeout(Duration::from_secs(15), probe)
            .await
            .expect("probe settlement completed")
            .expect("probe task")
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancelled_contained_probe() {
        cancel_blocked_probe("cmd.exe", vec!["/C", "cmd /C ping -n 30 127.0.0.1 >NUL"]).await;
        let outcome = cancel_during_probe_settlement(
            "cmd.exe",
            vec!["/D", "/C", "exit 0"],
            std::collections::VecDeque::from([
                ProbeNativeObservation::Active,
                ProbeNativeObservation::Active,
                ProbeNativeObservation::Active,
                ProbeNativeObservation::Error("injected query error"),
                ProbeNativeObservation::Empty,
            ]),
        )
        .await;
        let CancellableProbeOutcome::CleanupFailed(detail) = outcome else {
            panic!("expected later-settled cleanup failure, got {outcome:?}");
        };
        assert!(detail.contains("settlement attempt deadline exceeded during job accounting proof"));
        assert!(detail.contains("job accounting failed: injected query error"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_probe_kills_unix_process_group_and_drains_readers() {
        cancel_blocked_probe("sh", vec!["-c", "sh -c 'sleep 30' & wait"]).await;
        let outcome = cancel_during_probe_settlement(
            "sh",
            vec!["-c", "exit 0"],
            std::collections::VecDeque::from([
                ProbeNativeObservation::Active,
                ProbeNativeObservation::Active,
                ProbeNativeObservation::Active,
                ProbeNativeObservation::Error("EPERM"),
                ProbeNativeObservation::Empty,
            ]),
        )
        .await;
        let CancellableProbeOutcome::CleanupFailed(detail) = outcome else {
            panic!("expected later-settled cleanup failure, got {outcome:?}");
        };
        assert!(detail.contains(
            "settlement attempt deadline exceeded during process-group accounting proof"
        ));
        assert!(detail.contains("process-group accounting failed: EPERM"));
    }

    #[test]
    fn cache_lookup_ttl_and_single_flight() {
        let cache = Arc::new(AgentInstallCache::new());
        let now = Instant::now();
        assert!(matches!(
            cache.lookup("bob", now, INSTALL_CACHE_TTL),
            CacheLookup::Absent
        ));
        let ticket = cache.try_begin("bob").expect("ticket");
        assert!(matches!(
            cache.lookup("bob", now, INSTALL_CACHE_TTL),
            CacheLookup::InFlight
        ));
        let committed = ticket.complete(InstallState::missing("gone".to_string()));
        match committed {
            Completion::Committed(state) => assert_eq!(state.seq, 1),
            Completion::Stale(_) => panic!("unexpected stale completion"),
        }
        match cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL) {
            CacheLookup::Fresh(state) => {
                assert_eq!(state.status, InstallStatus::Missing);
                assert_eq!(state.seq, 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
        // Expired by a 0s TTL (an explicitly later `now` keeps this deterministic).
        let later = Instant::now() + Duration::from_secs(1);
        assert!(matches!(
            cache.lookup("bob", later, Duration::ZERO),
            CacheLookup::Absent
        ));
    }

    #[test]
    fn cache_commit_closes_the_ticket_once() {
        let cache = Arc::new(AgentInstallCache::new());
        let ticket = cache.try_begin("bob").expect("ticket");
        assert!(matches!(
            ticket.complete(InstallState::missing("gone".to_string())),
            Completion::Committed(_)
        ));
        let second = cache.try_begin("bob");
        assert!(second.is_some(), "the slot is free after a commit");
        assert!(
            cache.try_begin("bob").is_none(),
            "a second ticket for the same command must be refused"
        );
    }

    #[test]
    fn cache_lookup_or_begin_opens_exactly_one_ticket_under_contention() {
        let cache = Arc::new(AgentInstallCache::new());
        let barrier = std::sync::Barrier::new(8);
        let began = std::sync::Mutex::new(Vec::new());
        let in_flight_reports = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    match cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true) {
                        Scheduling::Began(ticket) => began.lock().expect("began lock").push(ticket),
                        Scheduling::InFlight => {
                            in_flight_reports.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        Scheduling::Fresh(_) => panic!("nothing was committed yet"),
                        Scheduling::Deferred => panic!("may_begin was true"),
                    }
                });
            }
        });
        let mut began = began.into_inner().expect("began lock");
        assert_eq!(began.len(), 1);
        assert_eq!(
            in_flight_reports.load(std::sync::atomic::Ordering::SeqCst),
            7
        );
        assert_eq!(cache.in_flight_len(), 1);
        let ticket = began.pop().expect("the one ticket");
        assert!(matches!(
            ticket.complete(InstallState::missing("gone".to_string())),
            Completion::Committed(_)
        ));
        match cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true) {
            Scheduling::Fresh(state) => assert_eq!(state.seq, 1),
            _ => panic!("a committed entry must be served fresh"),
        }
    }

    #[test]
    fn cache_lookup_or_begin_serves_a_commit_that_landed_before_the_second_call() {
        let cache = Arc::new(AgentInstallCache::new());
        let ticket = match cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true) {
            Scheduling::Began(ticket) => ticket,
            _ => panic!("first call must begin"),
        };
        let state = match ticket.complete(InstallState::missing("gone".to_string())) {
            Completion::Committed(state) => state,
            Completion::Stale(_) => panic!("unexpected stale completion"),
        };
        match cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true) {
            Scheduling::Fresh(served) => assert_eq!(served, state),
            _ => panic!("the second call must be served, never begin"),
        }
        assert_eq!(cache.in_flight_len(), 0);
    }

    #[test]
    fn cache_lookup_or_begin_defers_when_scheduling_is_not_allowed() {
        let cache = Arc::new(AgentInstallCache::new());
        assert!(matches!(
            cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, false),
            Scheduling::Deferred
        ));
        assert_eq!(cache.in_flight_len(), 0);
        let ticket = cache.try_begin("bob").expect("ticket");
        assert!(matches!(
            cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, false),
            Scheduling::InFlight
        ));
        assert!(matches!(
            ticket.complete(InstallState::missing("gone".to_string())),
            Completion::Committed(_)
        ));
        assert!(matches!(
            cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, false),
            Scheduling::Fresh(_)
        ));
    }

    #[test]
    fn cache_lookup_or_begin_replaces_an_expired_entry() {
        let cache = Arc::new(AgentInstallCache::new());
        let ticket = cache.try_begin("bob").expect("ticket");
        assert!(matches!(
            ticket.complete(InstallState::missing("gone".to_string())),
            Completion::Committed(_)
        ));
        let later = Instant::now() + Duration::from_secs(1);
        let renewed = match cache.lookup_or_begin("bob", later, Duration::ZERO, true) {
            Scheduling::Began(ticket) => ticket,
            _ => panic!("an expired entry must be replaced"),
        };
        assert!(matches!(
            cache.lookup("bob", later, Duration::ZERO),
            CacheLookup::InFlight
        ));
        drop(renewed);
    }

    #[test]
    fn cache_stale_completion_renews_ticket_and_blocks_competitors() {
        let cache = Arc::new(AgentInstallCache::new());
        let ticket = cache.try_begin("bob").expect("ticket");
        cache.invalidate_all();
        let renewed = match ticket.complete(InstallState::missing("first".to_string())) {
            Completion::Stale(renewed) => renewed,
            Completion::Committed(_) => panic!("an invalidated generation must be rejected"),
        };
        assert!(matches!(
            cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true),
            Scheduling::InFlight
        ));
        let second = InstallState::missing("second".to_string());
        let committed = match renewed.complete(second.clone()) {
            Completion::Committed(state) => state,
            Completion::Stale(_) => panic!("the renewed ticket must commit"),
        };
        assert_eq!(committed.seq, 1);
        assert_eq!(committed.detail.as_deref(), Some("second"));
        match cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL) {
            CacheLookup::Fresh(state) => assert_eq!(state, committed),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(matches!(
            cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true),
            Scheduling::Fresh(_)
        ));
    }

    #[test]
    fn cache_repeated_invalidation_rejects_twice_then_frees_the_slot() {
        let cache = Arc::new(AgentInstallCache::new());
        let ticket = cache.try_begin("bob").expect("ticket");
        cache.invalidate_all();
        let second = match ticket.complete(InstallState::missing("one".to_string())) {
            Completion::Stale(renewed) => renewed,
            Completion::Committed(_) => panic!("expected a stale completion"),
        };
        cache.invalidate_all();
        let third = match second.complete(InstallState::missing("two".to_string())) {
            Completion::Stale(renewed) => renewed,
            Completion::Committed(_) => panic!("expected a second stale completion"),
        };
        assert!(matches!(
            cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
            CacheLookup::InFlight
        ));
        assert!(matches!(
            cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true),
            Scheduling::InFlight
        ));
        drop(third);
        assert!(matches!(
            cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
            CacheLookup::Absent
        ));
        assert!(matches!(
            cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true),
            Scheduling::Began(_)
        ));
        assert_eq!(cache.seq(), 0);
        assert_eq!(cache.generation(), 2);
    }

    #[test]
    fn cache_seq_is_monotonic_across_invalidation() {
        let cache = Arc::new(AgentInstallCache::new());
        let ticket = cache.try_begin("bob").expect("ticket");
        match ticket.complete(InstallState::missing("one".to_string())) {
            Completion::Committed(state) => assert_eq!(state.seq, 1),
            Completion::Stale(_) => panic!("unexpected stale completion"),
        }
        cache.invalidate_all();
        let ticket = cache.try_begin("bob").expect("ticket");
        match ticket.complete(InstallState::missing("two".to_string())) {
            Completion::Committed(state) => assert_eq!(state.seq, 2),
            Completion::Stale(_) => panic!("unexpected stale completion"),
        }
        assert_eq!(cache.generation(), 1);
    }

    #[test]
    fn dropped_ticket_frees_in_flight() {
        let cache = Arc::new(AgentInstallCache::new());
        let ticket = cache.try_begin("bob").expect("ticket");
        assert_eq!(cache.in_flight_len(), 1);
        drop(ticket);
        assert_eq!(cache.in_flight_len(), 0);
        assert!(cache.try_begin("bob").is_some());
    }
}
