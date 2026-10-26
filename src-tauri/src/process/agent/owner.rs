//! Complete version-probe lifetime: bounded output, retained native ownership and settlement.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use regex::Regex;

/// Per-probe wall-clock cap. On expiry the whole tree is killed and the probe
/// reports `probeFailed`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Bytes retained per stream (the rest is drained and dropped).
pub const PROBE_OUTPUT_CAP: usize = 4 * 1024;
/// Sanitized diagnostic length, in characters.
pub const DETAIL_MAX_CHARS: usize = 160;
/// One cleanup attempt may spend this long proving the native tree empty. A
/// missed window records a defect but does not permit a terminal return; the
/// owned reaper starts another attempt and remains nonterminal until proof.
const ACTIVE_SETTLEMENT_WINDOW: Duration = Duration::from_secs(10);
const ACTIVE_SETTLEMENT_POLL: Duration = Duration::from_millis(25);

/// #1551 - remove every ANSI CSI/OSC sequence, then every remaining control
/// character, then trim. A lone `ESC` or an unterminated sequence swallows the
/// rest of the line (there is no cross-line state: each line stands alone).
pub(crate) fn strip_ansi_and_controls(line: &str) -> String {
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
    attempt: &mut crate::process::host::job::SettlementAttempt,
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
    probe_version_in(
        program,
        args,
        timeout,
        &crate::config::agent_path::effective_search_path(),
    )
    .await
}

/// #2589 - `probe_version` with an explicit PATH for the probe process, so an
/// agent whose shebang is `#!/usr/bin/env node` finds its interpreter.
pub async fn probe_version_in(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    search_path: &OsStr,
) -> ProbeOutcome {
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    match probe_version_cancellable(program, args, timeout, search_path, cancel_rx).await {
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
    search_path: &OsStr,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> CancellableProbeOutcome {
    let mut owner = RetainedProbeOwner::default();
    let outcome = probe_version_cancellable_retained(
        program,
        args,
        timeout,
        search_path,
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
#[allow(clippy::too_many_arguments)]
pub(crate) async fn probe_version_cancellable_retained<I, O>(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    search_path: &OsStr,
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
        c.env("PATH", search_path);
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

struct ProbeProcessOwner {
    child: Option<tokio::process::Child>,
    stdout: Option<RecoverableProbeReader>,
    stderr: Option<RecoverableProbeReader>,
    defects: Vec<String>,
    #[cfg(windows)]
    job: Option<crate::process::host::job::JobObject>,
    #[cfg(unix)]
    pgid: i32,
    #[cfg(test)]
    settlement_hook: Option<ProbeSettlementHook>,
}

impl ProbeProcessOwner {
    async fn spawn(command: &mut tokio::process::Command) -> Result<Self, CancellableProbeOutcome> {
        #[cfg(windows)]
        let (mut child, job) = match crate::process::host::job::spawn_suspended_contained(command)
            .await
        {
            Ok(pair) => pair,
            Err(crate::process::host::job::ContainedSpawnError::Spawn(error)) => {
                return Err(CancellableProbeOutcome::Completed(ProbeOutcome::Failed(
                    format!("spawn failed: {error}"),
                )))
            }
            Err(crate::process::host::job::ContainedSpawnError::Containment(reason)) => {
                log::warn!(
                    "[agent-version] contained probe launch rejected: \
                     process containment failed at {reason}"
                );
                return Err(CancellableProbeOutcome::Completed(ProbeOutcome::Failed(
                    "Version probe process-tree containment unavailable.".to_string(),
                )));
            }
            Err(crate::process::host::job::ContainedSpawnError::Cleanup(detail)) => {
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
        let mut attempt = crate::process::host::job::SettlementAttempt::new(settlement_window);
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
