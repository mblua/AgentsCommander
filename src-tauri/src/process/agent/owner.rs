//! Complete version-probe lifetime: bounded output, retained native ownership and settlement.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
mod command_run;
pub(crate) use command_run::{run_contained, ContainedRunEnd, ContainedRunSpec};

mod containment;
mod outcome;
pub(crate) use containment::RetainedProbeOwner;
pub use containment::PROBE_OUTPUT_CAP;
#[cfg(test)]
pub(crate) use containment::{
    with_probe_settlement_hook, with_probe_settlement_hook_after_spawns, ProbeNativeObservation,
    ProbeReaderSettlementHook, ProbeSettlementHook,
};
use containment::{ProbeProcessOwner, ProbeWait};
pub use outcome::{CancellableProbeOutcome, ProbeOutcome};

/// Per-probe wall-clock cap. On expiry the whole tree is killed and the probe
/// reports `probeFailed`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Sanitized diagnostic length, in characters.
pub const DETAIL_MAX_CHARS: usize = 160;

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
                if !skip_csi_chars(&mut chars) {
                    break;
                }
            }
            // OSC: everything up to and including BEL or the exact pair `ESC \`
            // (ST). An `ESC` followed by anything else is OSC content, so the
            // parser stays inside the sequence; it must not consume that next
            // character either, or it would eat the `ESC` of a later ST pair.
            Some(']') => {
                if !skip_osc_chars(&mut chars) {
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

fn skip_csi_chars(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut terminated = false;
    for c in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            terminated = true;
            break;
        }
    }
    terminated
}

fn skip_osc_chars(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
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
    terminated
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
