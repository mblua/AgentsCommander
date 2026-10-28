use super::notice::{
    logical_command_wire_value, parse_remote_pty_command, ResolvedRemotePtyCommand, RetryState,
    ERR_INVALID_DISPATCH_PROFILE, ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND, ERR_UNRESOLVABLE_AGENT,
    ERR_UNSUPPORTED_LOGICAL_REMOTE_COMMAND,
};
use crate::config::settings::AppSettings;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::path::PathBuf;

pub(crate) fn resolve_remote_pty_command(
    shell: &str,
    wire_value: &str,
) -> Result<ResolvedRemotePtyCommand, String> {
    let logical = parse_remote_pty_command(wire_value)?;
    let text =
        crate::pty::inject::resolve_logical_command_text(shell, logical).ok_or_else(|| {
            format!(
            "{} '{}': session shell '{}' has no verified mapping. Claude / Codex / Antigravity / \
            Cursor agent direct shells use /clear and /compact; exact Pi uses /new for clear only. \
            cmd / pwsh outer wrappers and Pi compact are unsupported.",
            ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND,
            logical_command_wire_value(logical),
            shell
        )
        })?;
    Ok(ResolvedRemotePtyCommand { logical, text })
}

pub(crate) const ERR_OWNED_WAKE_RETAINED: &str = "ownedWakeRetained:";

pub(crate) fn is_permanent_delivery_error(error: &str) -> bool {
    error.starts_with(ERR_OWNED_WAKE_RETAINED)
        || error.contains(ERR_UNRESOLVABLE_AGENT)
        || error.starts_with(ERR_UNSUPPORTED_LOGICAL_REMOTE_COMMAND)
        || error.starts_with(ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND)
        || error.starts_with(ERR_INVALID_DISPATCH_PROFILE)
}

/// (#1399) Claim marker for a message handed to a wake worker. Suffix, not a
/// subdirectory, so `move_to_delivered` / `reject_message` / the outbox-project
/// walk-up keep deriving from the same parent directory.
pub(crate) const WAKE_CLAIM_SUFFIX: &str = ".in-flight";

pub(crate) const WAKE_CLAIM_EXTENSION: &str = "in-flight";

/// (#1399) `<dir>/<id>.json` -> `<dir>/<id>.json.in-flight`. Appends to the
/// `OsString` of `file_name()` and uses `with_file_name`, never
/// `Path::with_extension`, so an id containing a dot cannot lose a component.
pub(crate) fn wake_claim_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(WAKE_CLAIM_SUFFIX);
    path.with_file_name(name)
}

/// (#1399) Inverse of `wake_claim_path`. `Some` only when the file name ends
/// with exactly `.json.in-flight`: reclamation must never resurrect a file this
/// poller could not have claimed. Strips from `file_name()`, never via
/// `with_extension`, so an id containing a dot survives; a non-UTF-8 name
/// yields `None`, the right answer for a file this poller did not write.
pub(crate) fn wake_claim_origin(claim: &Path) -> Option<PathBuf> {
    let name = claim.file_name()?.to_str()?;
    let origin = name.strip_suffix(WAKE_CLAIM_SUFFIX)?;
    if !origin.ends_with(".json") {
        return None;
    }
    Some(claim.with_file_name(origin))
}

pub(crate) fn wake_claim_has_settled_receipt(origin: &Path) -> Result<bool, String> {
    let Some(outbox_dir) = origin.parent() else {
        return Ok(false);
    };
    let Some(id) = origin.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(false);
    };
    let delivered = outbox_dir.join("delivered").join(format!("{}.json", id));
    if delivered.try_exists().map_err(|error| {
        format!(
            "could not inspect delivered payload receipt {:?}: {}",
            delivered, error
        )
    })? {
        return Ok(true);
    }
    let rejected = outbox_dir.join("rejected").join(format!("{}.json", id));
    rejected.try_exists().map_err(|error| {
        format!(
            "could not inspect rejected payload receipt {:?}: {}",
            rejected, error
        )
    })
}

/// (#1399) A failed restore leaves the durable claim in place. An owning drain
/// keeps and requeues its outcome; without ledger ownership, a later poll or
/// process restart retries general reclamation.
pub(crate) fn release_wake_claim(claim: &Path, origin: &Path) {
    if let Err(error) = std::fs::rename(claim, origin) {
        log::error!(
            "[mailbox] #1399 failed to return claim {:?} to {:?}: {}",
            claim,
            origin,
            error
        );
    }
}

/// (#1399) Send time for outbox ordering, or `None` when it cannot be read.
/// Never a filter: `None` sorts first so an unreadable or malformed document
/// still reaches the existing rejection path promptly.
///
/// Reads through `path_identity::read_bounded_regular` with the scan's own
/// byte cap, never a bare `std::fs::read`: the sort runs before classification
/// on every `*.json` in every outbox every cycle, so it must reach the same
/// verdict the hardened scan reader would (bounded, no-follow, regular files
/// only). Parsed as `serde_json::Value` because privileged PTY-input documents
/// share the outbox with `OutboxMessage` and both schemas carry a top-level
/// `timestamp`; parsed as RFC3339 rather than compared as bytes because
/// producers emit variable-width fractions and offsets.
pub(crate) fn outbox_message_send_time(path: &Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let (bytes, _identity) = crate::path_identity::read_bounded_regular(
        path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let raw = value.get("timestamp")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|sent| sent.with_timezone(&chrono::Utc))
}

/// (#1399) Collapse spellings of one physical outbox to a single entry, keyed
/// by file-object identity. Two spellings of one directory (case, separator,
/// 8.3 short name) would otherwise both be scanned, keep two independent
/// `retry_tracker` counters, and let reclamation under the second spelling
/// resurrect a claim owned under the first.
///
/// `is_dir()` FIRST. `verify_directory` is expensive (one `symlink_metadata`
/// per path component plus its own handle work, roughly 14 syscalls for a WG
/// replica outbox), the component walk stats every ancestor before the leaf
/// fails, and the large majority of registered projects have no outbox. The
/// cheap `is_dir()` is already the outbox loop's own first test, so filtering
/// on it here costs nothing and keeps thousands of stat calls off the 3 s
/// cycle this issue exists to make fast.
pub(crate) fn dedup_outbox_dirs_by_object_id(outbox_dirs: &mut Vec<PathBuf>) {
    outbox_dirs.retain(|dir| dir.is_dir());
    let mut seen = std::collections::HashSet::new();
    outbox_dirs.retain_mut(|dir| match crate::path_identity::verify_directory(dir) {
        Ok(identity) => {
            let canonical =
                crate::path_utils::normalize_windows_verbatim_path_buf(&identity.canonical_path);
            let is_new = seen.insert(identity.object_id);
            if is_new {
                *dir = canonical;
            }
            is_new
        }
        // Fail closed for this cycle: no raw alias may survive to a later
        // successful reclamation check while a worker owns the canonical key.
        Err(error) => {
            log::warn!(
                "[mailbox] #1399 skipping unverifiable outbox {:?} for this cycle: {}; scan and \
                reclamation deferred",
                dir,
                error
            );
            false
        }
    });
}

#[cfg(test)]
pub(crate) struct RetryPruneProbeBarrier {
    pub(crate) origin_absent: std::sync::Arc<std::sync::Barrier>,
    pub(crate) restore_complete: std::sync::Arc<std::sync::Barrier>,
}

/// (#1399) Drop retry state only after both durable source spellings were
/// successfully observed absent. An observation error is pending state, not
/// evidence that an already-counted delivery attempt disappeared.
pub(crate) fn prune_retry_tracker(
    retry_tracker: &mut std::collections::HashMap<PathBuf, RetryState>,
    live_claims: &std::collections::HashSet<PathBuf>,
    #[cfg(test)] probe_barrier: Option<&RetryPruneProbeBarrier>,
) {
    retry_tracker.retain(|path, _| match path.try_exists() {
        Ok(true) => true,
        Ok(false) => {
            let claim = wake_claim_path(path);
            #[cfg(test)]
            if let Some(probe_barrier) = probe_barrier {
                probe_barrier.origin_absent.wait();
                probe_barrier.restore_complete.wait();
            }
            if live_claims.contains(&claim) {
                true
            } else {
                match claim.try_exists() {
                    Ok(claim_present) => claim_present,
                    Err(error) => {
                        log::warn!(
                            "[mailbox] #1399 could not inspect retry claim {:?}: {}; retaining \
                            retry state",
                            claim,
                            error
                        );
                        true
                    }
                }
            }
        }
        Err(error) => {
            log::warn!(
                "[mailbox] #1399 could not inspect retry origin {:?}: {}; retaining retry state",
                path,
                error
            );
            true
        }
    });
}

/// (#1399 R1) The worker's outcome report, as a guard. `live_claims` is
/// released only by the drain, and the drain only runs for outcomes that
/// arrive, so the send must survive an unwind: without it, one panicking
/// worker (a poisoned `PtyManager` lock is a reachable production panic on
/// this path) leaves its claim owned forever, invisible to the scan AND
/// excluded from reclamation. Normal completion is byte-identical to a plain
/// `send`; a panicking or dropped worker reports a transient error instead of
/// reporting nothing.
pub(crate) struct WakeOutcomeReport {
    outcomes: tokio::sync::mpsc::UnboundedSender<(PathBuf, Result<(), String>)>,
    origin: Option<PathBuf>,
    outcome: Option<Result<(), String>>,
}

impl WakeOutcomeReport {
    pub(crate) fn new(
        outcomes: tokio::sync::mpsc::UnboundedSender<(PathBuf, Result<(), String>)>,
        origin: PathBuf,
    ) -> Self {
        Self {
            outcomes,
            origin: Some(origin),
            outcome: None,
        }
    }

    pub(crate) fn set(&mut self, outcome: Result<(), String>) {
        self.outcome = Some(outcome);
    }
}

impl Drop for WakeOutcomeReport {
    fn drop(&mut self) {
        let Some(origin) = self.origin.take() else {
            return;
        };
        // The synthesised error is deliberately transient: it matches none of
        // the `is_permanent_delivery_error` markers, so it counts an attempt
        // and retries, and only rejects after the attempt ceiling (10). Do not
        // edit the string casually in either direction.
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Err("wake worker ended without reporting an outcome".to_string()));
        // Discarded without panic if the receiver died with the poller at
        // shutdown; the claim on disk carries the message forward.
        let _ = self.outcomes.send((origin, outcome));
    }
}

/// (#1399) Outcome of trying to hand one validated message to a worker.
/// `LaneBusy` leaves the file untouched in the outbox for the next cycle;
/// `ClaimFailed` feeds the existing retry block as an ordinary transient error.
pub(crate) enum WakeHandoff {
    Spawned,
    LaneBusy,
    ClaimFailed(String),
}

/// #617 - sustained-idle window before a deferred provider-resolved logical
/// clear. `pub(crate)` so the CLI prose and response JSON single-source the
/// gate, the response `settle_secs`, and the CLI's conditional wording).
pub(crate) const SELF_CLEAR_SETTLE_SECS: u64 = 30;

// The next three are consumed only by the `#[cfg(not(test))]` spawn in
// `handle_self_clear`; test builds drive `run_self_clear_after_sustained_idle`
// with explicit durations, so they are dead under `cfg(test)`.
#[cfg_attr(test, allow(dead_code))]
pub(crate) const SELF_CLEAR_SETTLE: std::time::Duration =
    std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);

#[cfg_attr(test, allow(dead_code))]
pub(crate) const SELF_CLEAR_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// Safety cap so a never-idle session cannot leave a task polling forever.
/// Generous: any normal agent hits a 30s-idle window well within it.
#[cfg_attr(test, allow(dead_code))]
pub(crate) const SELF_CLEAR_MAX_DEFER: std::time::Duration = std::time::Duration::from_secs(3600);

/// #626 - the OutboxMessage `action` value for self-handoff-and-clear. Single-sourced so the CLI
/// emit, the early-dispatch match, and the response body cannot drift (a drift would make early
/// dispatch silently not fire and the command would be lost with no agent-visible error).
/// `pub(crate)` so
/// `cli/self_clear.rs` reaches it as `crate::phone::mailbox::SELF_CLEAR_ACTION`.
pub(crate) const SELF_CLEAR_ACTION: &str = "self-handoff-and-clear";

pub(crate) const RAISE_HAND_ACTION: &str = "raise-hand";

/// The canonical handoff filename at the agent root; also the prompt fallback when the
/// pre-inject archive did not happen (#749).
pub(crate) const SELF_HANDOFF_ROOT_NAME: &str = "SELF-HANDOFF.md";

/// Timestamp prefix for `self-clear/<timestamp>_<stem>.md` archives. Part of the naming
/// contract the #749 prompt and the CLI help expose to agents, so single-sourced.
pub(crate) const ARCHIVE_TIMESTAMP_FORMAT: &str = "%Y%m%d_%H%M%S";

/// #626/#749 - shared body of the Phase-2 resume prompt (the clear and switch variants differ
/// only in the opening clause). `handoff_path` is the file the agent must read, relative to its
/// root: the EXACT archived `self-clear/<ts>_SELF-HANDOFF.md` when the pre-inject archive
/// succeeded (#749), or the plain root `SELF-HANDOFF.md` when it did not (source absent or
/// rename failed). Must stay a SINGLE line (an embedded newline would submit early) and
/// self-contained (the agent's context was just wiped). Tests assert it is non-empty,
/// single-line, em-dash-free, and names the file in both the read instruction and the
/// missing-or-empty fallback clause.
pub(crate) fn handoff_base_prompt(event_clause: &str, handoff_path: &str) -> String {
    format!(
        "{event} To resume, read the file {p} relative to your own agent root (your current \
         working directory) and continue the work described there. If {p} is missing or empty, \
         wait for new instructions instead of guessing.",
        event = event_clause,
        p = handoff_path
    )
}

pub(crate) fn self_clear_handoff_base_prompt(handoff_path: &str) -> String {
    handoff_base_prompt(
        "Your context was just cleared by the self-handoff-and-clear command.",
        handoff_path,
    )
}

/// #668 - the OutboxMessage `action` value for self-handoff-and-switch.
pub(crate) const SELF_SWITCH_ACTION: &str = "self-handoff-and-switch";

/// #1632 - the OutboxMessage `action` value for self-handoff-and-restart. Single-sourced like the
/// other two so the CLI emit, the early-dispatch match, and the response body cannot drift.
pub(crate) const SELF_RESTART_ACTION: &str = "self-handoff-and-restart";

/// (#885) Bulk purge of the caller's own workgroup. Dispatched pre-routing,
/// like the other self-scoped actions: there is no single recipient to route to.
pub(crate) const PURGE_WG_ACTION: &str = "purge-wg";

/// #668/#749 - Phase-2 prompt for the switch variant; see `handoff_base_prompt`.
pub(crate) fn self_switch_handoff_base_prompt(handoff_path: &str) -> String {
    handoff_base_prompt(
        "Your session was just switched by the self-handoff-and-switch command.",
        handoff_path,
    )
}

/// #1632 - Phase-2 prompt for the restart variant; see `handoff_base_prompt`.
pub(crate) fn self_restart_handoff_base_prompt(handoff_path: &str) -> String {
    handoff_base_prompt(
        "Your session was just restarted by the self-handoff-and-restart command.",
        handoff_path,
    )
}

pub(crate) const SELF_FORGET_SUMMARY_MAX_CHARS: usize = 240;

pub(crate) const SELF_FORGET_SUMMARY_READ_LIMIT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForgottenSummary(String);

impl ForgottenSummary {
    pub(crate) fn from_raw(raw: &str) -> Option<Self> {
        let mut parts = Vec::new();

        for line in raw.lines() {
            if let Some(normalized) = normalize_self_forget_line(line) {
                parts.push(normalized);
            }
        }

        let collapsed = parts.join("; ");
        let collapsed = collapsed.trim();
        if collapsed.is_empty() {
            return None;
        }

        let char_count = collapsed.chars().count();
        if char_count <= SELF_FORGET_SUMMARY_MAX_CHARS {
            return Some(Self(collapsed.to_string()));
        }

        let mut truncated: String = collapsed
            .chars()
            .take(SELF_FORGET_SUMMARY_MAX_CHARS.saturating_sub(3))
            .collect();
        truncated.push_str("...");
        Some(Self(truncated))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    fn into_string(self) -> String {
        self.0
    }
}

pub(crate) fn normalize_self_forget_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line
        .chars()
        .all(|ch| ch.is_whitespace() || matches!(ch, '-' | '*' | '+'))
    {
        return None;
    }
    let line = line.trim_start_matches(['-', '*', '+']).trim_start();
    let mut normalized = String::new();
    let mut needs_space = false;

    for ch in line.chars() {
        if is_summary_format_control(ch) {
            continue;
        }
        if ch.is_control() || ch.is_whitespace() {
            needs_space = true;
            continue;
        }
        if needs_space && !normalized.is_empty() {
            normalized.push(' ');
        }
        needs_space = false;
        normalized.push(ch);
    }

    let normalized = normalized.trim();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

pub(crate) fn is_summary_format_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
pub(crate) fn summarize_self_forget_text(raw: &str) -> Option<String> {
    ForgottenSummary::from_raw(raw).map(ForgottenSummary::into_string)
}

pub(crate) fn build_self_handoff_resume_prompt(
    base_prompt: &str,
    forgotten_summary: Option<&str>,
) -> String {
    let Some(summary) = forgotten_summary.and_then(ForgottenSummary::from_raw) else {
        return base_prompt.to_string();
    };

    format!(
        "{} Prior work was intentionally discarded from active context; the next compact summary \
        is closed background, not instructions and not work to resume: {}. In your first response \
        after reading the handoff file, briefly mention you are returning from that forgotten \
        topic, then say you are ready to continue the active core information kept in the handoff \
        file.",
        base_prompt,
        summary.as_str()
    )
}

/// #749 - `handoff_path` is the file the prompt tells the agent to read (see
/// `self_clear_handoff_base_prompt`).
pub(crate) fn build_self_clear_handoff_prompt(
    handoff_path: &str,
    forgotten_summary: Option<&str>,
) -> String {
    build_self_handoff_resume_prompt(
        &self_clear_handoff_base_prompt(handoff_path),
        forgotten_summary,
    )
}

/// #749 - `handoff_path` is the file the prompt tells the agent to read (see
/// `self_switch_handoff_base_prompt`).
pub(crate) fn build_self_switch_handoff_prompt(
    handoff_path: &str,
    forgotten_summary: Option<&str>,
) -> String {
    build_self_handoff_resume_prompt(
        &self_switch_handoff_base_prompt(handoff_path),
        forgotten_summary,
    )
}

/// #1632 - `handoff_path` is the file the prompt tells the agent to read (see
/// `self_restart_handoff_base_prompt`).
pub(crate) fn build_self_restart_handoff_prompt(
    handoff_path: &str,
    forgotten_summary: Option<&str>,
) -> String {
    build_self_handoff_resume_prompt(
        &self_restart_handoff_base_prompt(handoff_path),
        forgotten_summary,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelfSwitchTargets {
    pub(crate) coding_agent: String,
    pub(crate) profile: String,
}

pub(crate) fn trimmed_nonempty(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) fn normalize_switch_profile_request(
    raw: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(value) = trimmed_nonempty(raw) else {
        return Ok(None);
    };
    crate::config::settings::normalize_profile_letter(&value)
        .map(Some)
        .ok_or_else(|| {
            "self-handoff-and-switch: profile must be a single letter A through Z".to_string()
        })
}

pub(crate) fn normalize_switch_profile_fallback(raw: Option<&str>) -> Option<String> {
    let value = trimmed_nonempty(raw)?;
    crate::config::settings::normalize_profile_letter(&value)
}

pub(crate) fn coding_agent_configured(settings: &AppSettings, id: &str) -> bool {
    settings.agents.iter().any(|agent| agent.id == id)
}

pub(crate) fn configured_coding_agent_choices(settings: &AppSettings) -> String {
    if settings.agents.is_empty() {
        return "<none configured>".to_string();
    }
    settings
        .agents
        .iter()
        .map(|agent| format!("{} ({})", agent.id, agent.label))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn unknown_coding_agent_message(settings: &AppSettings, id: &str) -> String {
    format!(
        "self-handoff-and-switch: coding agent '{}' is not configured. Configured coding agents: \
        {}",
        id,
        configured_coding_agent_choices(settings)
    )
}

/// #1632 - the per-flavor strings the shared respawn driver needs. Switch and restart differ only
/// in their log label, their phase-1 verb, their phase-2 verb, and their resume-prompt builder;
/// everything else, the two-phase gate, the pending-set alias bookkeeping, and the archive-first
/// phase-2 tail, is identical and must not fork.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SelfHandoffFlavor {
    pub(crate) action: &'static str,
    pub(crate) phase1_verb: &'static str,
    pub(crate) phase2_verb: &'static str,
    pub(crate) build_prompt: fn(&str, Option<&str>) -> String,
}

pub(crate) const SELF_HANDOFF_FLAVOR_SWITCH: SelfHandoffFlavor = SelfHandoffFlavor {
    action: SELF_SWITCH_ACTION,
    phase1_verb: "persisting the target selection and respawning",
    phase2_verb: "post-switch",
    build_prompt: build_self_switch_handoff_prompt,
};

pub(crate) const SELF_HANDOFF_FLAVOR_RESTART: SelfHandoffFlavor = SelfHandoffFlavor {
    action: SELF_RESTART_ACTION,
    phase1_verb: "respawning the live recipe",
    phase2_verb: "post-restart",
    build_prompt: build_self_restart_handoff_prompt,
};

/// #1632 - the live recipe a restart must reproduce. `agent` is required: with no coding-agent
/// identity there is nothing to pin, and passing None would hand the choice back to
/// `resolve_restart_selected_agent_id`, which ranks the replica `currentCodingAgent` cell ABOVE
/// the agent the live session was launched with (commands/session.rs:3626-3641). `profile` is
/// optional: when the session inherited its letter, None reproduces exactly the resolution its
/// own spawn used, and no letter is invented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelfRestartRecipe {
    pub(crate) agent: String,
    pub(crate) profile: Option<String>,
}

pub(crate) fn resolve_self_restart_recipe(
    session_agent: Option<&str>,
    session_effective_profile: Option<&str>,
    session_requested_profile: Option<&str>,
) -> Result<SelfRestartRecipe, String> {
    let agent = trimmed_nonempty(session_agent).ok_or_else(|| {
        "self-handoff-and-restart: this session has no configured coding-agent identity, so there \
         is no recipe to respawn; run it from a coding-agent session."
            .to_string()
    })?;
    let profile = normalize_switch_profile_fallback(session_effective_profile)
        .or_else(|| normalize_switch_profile_fallback(session_requested_profile));
    Ok(SelfRestartRecipe { agent, profile })
}

/// #1632 - the driver seam carries the profile as a `String` (the `Restart` closure's second
/// profile argument), so an absent profile letter must cross it as a sentinel. The empty string is
/// that sentinel: it is not a valid profile (`normalize_switch_profile_fallback` rejects it) so it
/// cannot collide with a real letter. These two are inverses and are pinned as such by T20;
/// keeping them adjacent and named is what stops a future edit from encoding here and decoding
/// somewhere else.
pub(crate) fn encode_restart_profile_for_seam(profile: Option<&str>) -> String {
    profile.unwrap_or("").to_string()
}

pub(crate) fn decode_restart_profile_from_seam(profile: &str) -> Option<String> {
    trimmed_nonempty(Some(profile))
}

/// #1632 - operator-facing rendering of the same seam value. The sentinel must never reach a log
/// line as `profile ''`. Third member of the sentinel's family, so it lives with its inverses
/// rather than as an inline conditional in the shared driver body.
pub(crate) fn display_profile_for_log(profile: &str) -> &str {
    if profile.trim().is_empty() {
        "<inherited>"
    } else {
        profile
    }
}

pub(crate) fn resolve_switch_targets(
    settings: &AppSettings,
    cwd: &Path,
    req_agent: Option<&str>,
    req_profile: Option<&str>,
    session_agent: Option<&str>,
    session_effective_profile: Option<&str>,
    session_requested_profile: Option<&str>,
) -> Result<SelfSwitchTargets, String> {
    let coding_agent = if let Some(requested) = trimmed_nonempty(req_agent) {
        if !coding_agent_configured(settings, &requested) {
            return Err(unknown_coding_agent_message(settings, &requested));
        }
        Some(requested)
    } else if let Some(live) = trimmed_nonempty(session_agent) {
        Some(live)
    } else {
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(cwd)
            .and_then(|value| trimmed_nonempty(Some(value.as_str())))
            .filter(|value| coding_agent_configured(settings, value))
    };
    let coding_agent = coding_agent.ok_or_else(|| {
        format!(
            "self-handoff-and-switch: no target coding agent could be resolved. Pass \
            --coding-agent <id>. Configured coding agents: {}",
            configured_coding_agent_choices(settings)
        )
    })?;

    let profile = normalize_switch_profile_request(req_profile)?
        .or_else(|| normalize_switch_profile_fallback(session_effective_profile))
        .or_else(|| normalize_switch_profile_fallback(session_requested_profile))
        .or_else(|| crate::config::coding_agent_profiles::read_replica_profile(cwd))
        .unwrap_or_else(|| "A".to_string());

    Ok(SelfSwitchTargets {
        coding_agent,
        profile,
    })
}

pub(crate) fn validate_self_switch_wg_replica(
    settings: &AppSettings,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let name = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
            "self-handoff-and-switch is only supported from a Room replica (__agent_* under room-* \
            or legacy wg-*); '{}' has no valid final path component",
            cwd.display()
        )
        })?;
    if !name.starts_with("__agent_") {
        return Err(format!(
            "self-handoff-and-switch is only supported from a Room replica (__agent_* under room-* \
            or legacy wg-*); got '{}'",
            cwd.display()
        ));
    }
    let validated =
        crate::config::coding_agent_profiles::validate_profile_selection_agent_path(settings, cwd)
            .map_err(|e| {
                format!(
                    "self-handoff-and-switch is only supported from a configured Room replica: {}",
                    e
                )
            })?;
    let validated_name = validated
        .launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !validated_name.starts_with("__agent_") {
        return Err(format!(
            "self-handoff-and-switch is only supported from a Room replica; validated target was \
            '{}'",
            validated.launch_path.display()
        ));
    }
    Ok(validated.launch_path)
}

pub(crate) fn validate_self_switch_spawn(
    settings: &AppSettings,
    cwd: &Path,
    targets: &SelfSwitchTargets,
) -> Result<(), String> {
    if !coding_agent_configured(settings, &targets.coding_agent) {
        return Err(unknown_coding_agent_message(
            settings,
            &targets.coding_agent,
        ));
    }
    let profile =
        crate::config::settings::normalize_profile_letter(&targets.profile).ok_or_else(|| {
            "self-handoff-and-switch: profile must be a single letter A through Z".to_string()
        })?;
    crate::config::agent_command::build_agent_spawn_command(
        settings,
        &targets.coding_agent,
        Some(cwd),
        Some(&profile),
    )
    .map(|_| ())
    .map_err(|e| {
        format!(
            "self-handoff-and-switch: target coding agent '{}' profile '{}' is not launchable from \
            '{}': {}",
            targets.coding_agent,
            profile,
            cwd.display(),
            e
        )
    })
}

/// §DR5 anti-spoof accept rule. Outbox-sender check passes when `msg_from`
/// equals `expected_from` exactly, OR when `msg_from` is unqualified (legacy)
/// AND its local part matches `expected_from`'s local part. Qualified-but-
/// wrong-project `msg_from` is always rejected.
pub(crate) fn anti_spoof_accept(msg_from: &str, expected_from: &str) -> bool {
    if msg_from == expected_from {
        return true;
    }
    let (_, exp_local) = crate::config::teams::split_project_prefix(expected_from);
    let (msg_proj, msg_local) = crate::config::teams::split_project_prefix(msg_from);
    msg_proj.is_none() && exp_local == msg_local
}

/// §AR2-norm step (1): upgrade a legacy-unqualified `msg_from` to
/// `expected_from` when expected_from is FQN. Returns true if the upgrade
/// happened. No-op when msg_from is already qualified, expected_from is
/// None, or expected_from itself is unqualified.
pub(crate) fn canonicalize_msg_from_in_place(
    msg_from: &mut String,
    expected_from: Option<&str>,
) -> bool {
    let Some(exp) = expected_from else {
        return false;
    };
    let (exp_proj, _) = crate::config::teams::split_project_prefix(exp);
    if exp_proj.is_none() {
        return false;
    }
    let (msg_proj, _) = crate::config::teams::split_project_prefix(msg_from);
    if msg_proj.is_some() {
        return false;
    }
    *msg_from = exp.to_string();
    true
}

/// Decision made by `deliver_wake` for an existing session.
#[derive(Debug, PartialEq)]
pub(crate) enum WakeAction {
    /// Session is live — inject into stdin, regardless of whether the agent
    /// is waiting for input or mid-turn. Bias toward delivery.
    Inject,
    /// Session is Exited — destroy it and fall through to spawn a fresh one.
    RespawnExited,
}

/// Pure decision given a session's status. Extracted so the decision table is
/// unit-testable without a tauri runtime. `deliver_wake` calls this and acts
/// on the result; any future restoration of a busy-gate would require editing
/// this fn (and its tests below), not a lone `if` inside `deliver_wake`.
pub(crate) fn wake_action_for(status: &SessionStatus) -> WakeAction {
    if matches!(status, SessionStatus::Exited(_)) {
        WakeAction::RespawnExited
    } else {
        WakeAction::Inject
    }
}

/// `skip_auto_resume` value used by `deliver_wake`'s spawn-fallback. Inverts
/// the positive-form `spawn_with_resume` flag so call sites read naturally.
///
/// Pinned via this helper to fence against a future refactor that "simplifies"
/// `!spawn_with_resume` to `spawn_with_resume` and silently regresses #82.
/// See plan §8.2 / round-2 R2.7 / round-3 R3.2.
pub(crate) fn wake_spawn_skip_auto_resume(spawn_with_resume: bool) -> bool {
    !spawn_with_resume
}

/// #2411 Internal-system wake resume rule: a surviving exited record resumes
/// (unchanged); with no record, resume only if idle auto-close closed it.
pub(crate) fn internal_wake_resumes(had_exited_record: bool, auto_closed: bool) -> bool {
    had_exited_record || auto_closed
}

/// Decide whether a session is a viable candidate for the mailbox to attempt
/// delivery to. Pure function — unit-testable without a tauri runtime.
///
/// Rules:
/// - `Exited(_)` records are KEPT even with `has_pty == false` — the wake
///   contract documents the respawn path: "if Exited, respawn". The respawn
///   path does NOT need a live PTY (it calls `destroy_session_inner` then
///   `create_session_inner`).
/// - All other statuses (`Active`/`Running`/`Idle`) require `has_pty == true`.
///   A SessionManager record with one of these statuses but no PtyManager
///   entry is a desync phantom — the inject path is guaranteed to fail with
///   `AppError::SessionNotFound`, so the router must skip it (issue #223).
///
/// Exhaustive `match` (not `matches!`) — a future `SessionStatus` variant
/// forces a deliberate compile-error decision rather than silently routing
/// to the `has_pty` branch. (dev-rust R1.B7 / grinch G.M6.)
pub(crate) fn is_viable_wake_candidate(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => true,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

/// Decide whether a `SessionInfo` candidate is a viable Root Agent recipient
/// for delivery. Stricter than `is_viable_wake_candidate`: rejects
/// `SessionStatus::Exited(_)` outright.
///
/// The Root Agent is user-launched and never auto-respawned (#293 §3 contract).
/// `deliver_wake`'s deferred-destroy block fires for any candidate that hit
/// the `RespawnExited` arm, which would silently destroy the user's Root
/// Agent session record before the no-spawn guard fires. Returning `false`
/// for Exited keeps the record in place so the user sees it in the sidebar
/// exactly as they left it.
///
/// Exhaustive `match` (not `matches!`) — a future `SessionStatus` variant
/// forces a deliberate compile-error decision. Same pattern as
/// `is_viable_wake_candidate`.
pub(crate) fn is_viable_root_recipient(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => false,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

/// One tick of the #611 sustained-idle gate for a freshly spawned wake session.
///
/// A cold-starting agent (notably Claude) can hit a quiet window longer than the
/// idle threshold mid-startup and be marked `waiting_for_input` before its TUI
/// input/paste state is stable. Injecting then leaves the body in the input box
/// but the submit `\r` can be dropped, so the message is written yet never sent.
/// `wait_for_spawned_wake_idle` therefore requires idle to hold continuously for
/// a settle window before injecting; the existing double-Enter in `pty/inject.rs`
/// then submits reliably once the agent is stable.
///
/// Pure so the settle/reset policy is unit-testable without timers. Threads
/// `idle_since` (the instant the session was first seen continuously idle, or
/// `None` if it is currently busy / has not yet been observed idle) and returns
/// the next `idle_since` plus whether idle has now held for `settle`:
/// - not ready (`waiting_for_input == false`) resets the clock to `None`: a late
///   startup render restarts the settle window. #1388: the cold-spawn caller now
///   passes the composed `wake_settle_ready`, so `false` also means "idle with a
///   blank screen", and the reset makes the settle window measure idle-AND-rendered
///   held continuously.
/// - idle starts (or keeps) the clock; once `now - idle_since >= settle` the
///   caller may inject.
pub(crate) fn next_sustained_idle_state(
    waiting_for_input: bool,
    idle_since: Option<std::time::Instant>,
    now: std::time::Instant,
    settle: std::time::Duration,
) -> (Option<std::time::Instant>, bool) {
    if !waiting_for_input {
        return (None, false);
    }
    let since = idle_since.unwrap_or(now);
    // checked_duration_since guards against a non-monotonic `now` (parity with
    // idle_detector's threshold check); treat an unexpected backwards clock as
    // "not yet settled" rather than panicking.
    let settled = now
        .checked_duration_since(since)
        .map(|elapsed| elapsed >= settle)
        .unwrap_or(false);
    (Some(since), settled)
}

/// (#1001 PR2 / B) Extra settle beyond `idle_threshold` for the live-inject gate.
/// A session only reports `waiting_for_input == true` after `idle_threshold`
/// (~2500ms) of quiet, so a settle <= `idle_threshold` never bites (grinch G3).
/// The guard makes B gate exactly the `[idle_threshold, idle_threshold +
/// FRESH_IDLE_GUARD]` window where a just-idle TUI may not be paste-ready yet.
pub(crate) const FRESH_IDLE_GUARD: std::time::Duration = std::time::Duration::from_millis(1000);

/// (#1001 PR2 / B) The action `settle_until_ready` takes on one tick.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SettleAction {
    /// Inject now: busy fast-path, sustained idle reached, or the max_wait cap
    /// (never drop a delivery).
    InjectNow,
    /// Keep waiting; sleep one poll and re-check.
    Wait,
}
