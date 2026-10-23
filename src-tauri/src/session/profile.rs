//! Per-coding-agent profile — the single source of truth for behavior that
//! varies by coding agent (Claude Code, Codex CLI, Antigravity CLI, Pi Coding Agent).
//!
//! Before #260 this knowledge was scattered: three `is_claude`/`is_codex`/
//! `is_gemini` bools on `Session`/`SessionInfo` (#258), a duplicated
//! `starts_with` detector in `create_session_inner` and
//! `strip_auto_injected_args`, the `derive_reader` bool triple, and
//! hard-coded idle-detector thresholds. `CodingAgentProfile` consolidates it.
//!
//! Design (see _plans/260-coding-agent-profile.md §2): plain `Copy` data +
//! `const` lookup, not a trait — the agent set is small and closed and only
//! data varies, so a struct beats a `dyn` object (no vtables, no allocation,
//! usable in `const` context, exhaustive `match` on `CodingAgentKind`).

use std::time::Duration;

use serde::{Deserialize, Serialize};

mod pi_command;
mod pty_submission;

use pi_command::is_exact_muse_executable;
pub(crate) use pi_command::locate_pi_command;
#[cfg(test)]
use pi_command::PiCommandLocation;
#[cfg(test)]
use pi_command::PiCommandParseError;
pub(crate) use pi_command::PiInsertionPoint;
pub(crate) use pty_submission::detect_configured_pty_submission_agent;
pub use pty_submission::detect_pty_submission_agent;
#[cfg(test)]
use pty_submission::detect_pty_submission_agent_with_provenance;
pub use pty_submission::PtySubmissionAgent;

/// Identity of a coding agent. `Option<CodingAgentKind>` on a session: `None`
/// means "not a recognised coding agent" (a plain shell).
///
/// Mutual exclusion is **structural** — a session is exactly one kind or none.
/// This enum is what let #260 delete the `debug_assert!` that guarded the old
/// three-bool representation in `derive_reader`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentKind {
    Claude,
    Codex,
    Antigravity,
    Pi,
    /// #1873 - Muse Code. Detected only as a direct shell whose exact stem is
    /// `muse` (bare or absolute path); prefix names and argument-only identity
    /// inside `env`/shell/PowerShell/`cmd`/compound wrappers are never Muse.
    Muse,
}

impl CodingAgentKind {
    /// Detect the coding agent from a spawn command (`shell` + `args`).
    ///
    /// First applies Pi's exact command-position parser. A supported Pi head
    /// wins over provider-looking option values, while malformed or reserved
    /// unsupported Pi shapes fail closed. Only a genuine non-Pi result reaches
    /// the legacy whitespace/basename prefix scan with precedence
    /// **Claude > Codex > Antigravity**. Claude and Codex keep their legacy
    /// prefix match for wrappers such as `claude-mb` and `codex-foo`;
    /// Antigravity matches the exact stems `agy` | `antigravity` on the
    /// already-stemmed lowercase basename (`agy.exe`/`agy.cmd`/`agy.bat`/
    /// `antigravity.exe` collapse via `file_stem`); prefix wrappers are not
    /// inferred. A user-defined `gemini*` command now gets generic behavior.
    ///
    /// THIS IS THE detector. `create_session_inner` (which stamps
    /// `Session::agent_kind`) and `strip_auto_injected_args` both call it, so
    /// the persisted recipe and the runtime identity can never disagree.
    pub fn detect(shell: &str, args: &[String]) -> Option<CodingAgentKind> {
        match locate_pi_command(shell, args) {
            Ok(Some(_)) => return Some(CodingAgentKind::Pi),
            Ok(None) => {}
            Err(_) => return None,
        }

        // #1873 - Muse is a direct-shell-only identity and outranks the legacy
        // token scans below, so a Codex-looking argument value such as
        // `muse --workspace /tmp/codex` stays Muse. Wrappers (`env muse`,
        // `cmd /C muse`) never reach here as Muse: the shell is the wrapper.
        if is_exact_muse_executable(shell) {
            return Some(CodingAgentKind::Muse);
        }

        // Mirror of `crate::commands::session::executable_basename`
        // (`session.rs:1506`, identical body). Deliberately NOT shared:
        // importing it would invert the dependency direction — the `session`
        // domain module would depend on the `commands` (IPC) layer (§2 D2).
        // ~6 trivial lines; do not "consolidate" into a layering violation
        // (dev-rust R1.4 #3).
        fn basename(token: &str) -> String {
            std::path::Path::new(token)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(token)
                .to_lowercase()
        }
        let basenames: Vec<String> = std::iter::once(shell.to_string())
            .chain(
                args.iter()
                    .flat_map(|a| a.split_whitespace().map(str::to_string)),
            )
            .map(|t| basename(&t))
            .collect();
        // Precedence claude > codex > antigravity, scanning every token.
        if basenames.iter().any(|b| b.starts_with("claude")) {
            Some(CodingAgentKind::Claude)
        } else if basenames.iter().any(|b| b.starts_with("codex")) {
            Some(CodingAgentKind::Codex)
        } else if basenames
            .iter()
            .any(|b| matches!(b.as_str(), "agy" | "antigravity"))
        {
            Some(CodingAgentKind::Antigravity)
        } else {
            None
        }
    }

    /// Stable lowercase name of the CLI, for logs and for keying diagnostics by the
    /// coding agent that is actually running (#942). Not the configured profile id.
    pub const fn as_str(self) -> &'static str {
        match self {
            CodingAgentKind::Claude => "claude",
            CodingAgentKind::Codex => "codex",
            CodingAgentKind::Antigravity => "agy",
            CodingAgentKind::Pi => "pi",
            CodingAgentKind::Muse => "muse",
        }
    }

    /// Resolve the full behavior profile for this kind.
    pub const fn profile(self) -> CodingAgentProfile {
        match self {
            CodingAgentKind::Claude => CLAUDE_PROFILE,
            CodingAgentKind::Codex => CODEX_PROFILE,
            CodingAgentKind::Antigravity => ANTIGRAVITY_PROFILE,
            CodingAgentKind::Pi => PI_PROFILE,
            CodingAgentKind::Muse => MUSE_PROFILE,
        }
    }
}

/// #2879 - which activity detector arbitrates a session's busy state. `Timed` is the
/// pre-#2879 silence clock alone; `CodexWorking` additionally keeps the session busy while
/// Codex's complete `Working (… • esc to interrupt)` indicator is visible. Selected per
/// profile only: reverting `CODEX_PROFILE` to `Timed` reverts the feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityDetectorKind {
    Timed,
    CodexWorking,
}

/// Per-session tuning for the PTY idle detector. Resolved from the session's
/// `CodingAgentProfile` (or `DEFAULT` for a plain shell) and handed to
/// `IdleDetector::register_session` at PTY spawn time.
///
/// Invariant: `resize_grace >= idle_threshold` — a resize repaint must not be
/// able to trigger a false busy→idle transition. `register_session`
/// `debug_assert!`s it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleTuning {
    /// PTY silence after which a session is reported idle / waiting-for-input.
    pub idle_threshold: Duration,
    /// Grace window after a resize during which PTY output is treated as
    /// prompt-repaint noise and does NOT reset the idle timer.
    pub resize_grace: Duration,
    /// #260 BUG FIX. When `true`, `IdleDetector::register_session` seeds
    /// `activity[id] = Instant::now()` at PTY spawn. Without this seed, a
    /// session whose entire visible output is suppressed (resize grace) or
    /// escape-only (SKIPPED) is never inserted into the detector's `activity`
    /// map, so the watcher thread — which only iterates `activity` — never
    /// evaluates it and `mark_idle` never fires. See plan §1.
    pub seed_initial_activity: bool,
    /// #2124 - byte total at which a pending output burst is confirmed as real
    /// work. `0` (or a zero `burst_window`) disables the filter for this
    /// session and restores the pre-#2124 behavior exactly. #2156 - ON by
    /// default for a recognised coding agent ([`IdleTuning::AGENT_DEFAULT`]);
    /// OFF for a plain shell ([`IdleTuning::DEFAULT`]).
    pub burst_max_bytes: u64,
    /// #2124 - longest gap a pending burst may keep spanning. A burst still
    /// unconfirmed `burst_window` after its first chunk is discarded.
    pub burst_window: Duration,
    /// #2124 - silence age a chunk must find before it may OPEN a burst
    /// candidate. Shorter silences count immediately, so input and message
    /// delivery still register right away.
    pub burst_prior_silence: Duration,
    /// #2879 - the activity detector for this session. Launch overrides only touch the burst
    /// fields, so this selector always survives `idle_tuning_for_launch`.
    pub(crate) activity_detector: ActivityDetectorKind,
}

/// #2124 defaults for an `idleBurst` object that omits a subfield.
const IDLE_BURST_DEFAULT_MAX_BYTES: u64 = 1024;
const IDLE_BURST_DEFAULT_MAX_SECS: f64 = 3.0;
const IDLE_BURST_DEFAULT_PRIOR_SILENCE_SECS: f64 = 60.0;
/// Caps that keep `Duration::from_secs_f64` panic-free for absurd catalog values.
const IDLE_BURST_MAX_WINDOW_SECS: f64 = 600.0;
const IDLE_BURST_MAX_PRIOR_SILENCE_SECS: f64 = 86_400.0;

impl IdleTuning {
    /// Tuning for a plain shell / unrecognised agent (`idle_tuning_for(None)`).
    /// Also the per-field fallback when a session id is missing from the
    /// detector's tuning map. Values are identical to the pre-#260
    /// `idle_detector.rs` constants, with the #2124 burst filter disabled.
    /// A recognised coding agent uses [`IdleTuning::AGENT_DEFAULT`] instead.
    pub const DEFAULT: IdleTuning = IdleTuning {
        idle_threshold: Duration::from_millis(2500),
        resize_grace: Duration::from_millis(3000),
        seed_initial_activity: true,
        burst_max_bytes: 0,
        burst_window: Duration::ZERO,
        burst_prior_silence: Duration::ZERO,
        activity_detector: ActivityDetectorKind::Timed,
    };

    /// #2156 - tuning for a recognised coding agent: `DEFAULT` plus the #2124
    /// burst filter ON by default. Values mirror the IDLE_BURST_DEFAULT_*
    /// constants (a test pins the two in sync).
    pub const AGENT_DEFAULT: IdleTuning = IdleTuning {
        burst_max_bytes: IDLE_BURST_DEFAULT_MAX_BYTES, // 1024
        burst_window: Duration::from_millis(3_000),    // IDLE_BURST_DEFAULT_MAX_SECS
        burst_prior_silence: Duration::from_secs(60),  // IDLE_BURST_DEFAULT_PRIOR_SILENCE_SECS
        ..IdleTuning::DEFAULT
    };
}

/// #930 - per-coding-agent host credential source for container copy-in.
/// Static data only (all `&'static str`), so it is usable in the `const`
/// profile table and carries no host-specific paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerCredentialSource {
    /// Default host config dir, relative to the user's home dir, e.g. ".claude".
    pub host_dir: &'static str,
    /// Optional env var whose value (an absolute path) overrides `host_dir` on
    /// the host, e.g. "CLAUDE_CONFIG_DIR" for Claude. Only the AgentsCommander
    /// process environment is consulted (NOT wrapper-script parsing).
    pub host_dir_env: Option<&'static str>,
    /// Credential filename within the config dir, e.g. ".credentials.json".
    pub file: &'static str,
    /// Container-side config dir relative to the bind-mount root (`host_root`),
    /// e.g. ".claude". Where the file is copied so the container reads it.
    pub container_dir: &'static str,
    /// #930 - first-run state stamped next to the copied credential so the
    /// agent's interactive TUI actually USES it instead of running its
    /// onboarding wizard. None = nothing is stamped for this agent.
    pub first_run: Option<ContainerFirstRunState>,
}

/// #930 - container-side first-run state a coding agent needs before its
/// interactive TUI will use a copied credential. Verified in a real container:
/// Claude Code gates its onboarding wizard on `hasCompletedOnboarding` in
/// `$CLAUDE_CONFIG_DIR/.claude.json`, and the folder-trust dialog on
/// `projects[<cwd>].hasTrustDialogAccepted`, checking NEITHER against the
/// credential file. So a valid copied token still lands on "Select login
/// method" unless these flags are set. Static data only (all `&'static str`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerFirstRunState {
    /// JSON config file inside `container_dir`, e.g. ".claude.json".
    pub file: &'static str,
    /// Top-level boolean gating the onboarding wizard, set to `true`, e.g.
    /// "hasCompletedOnboarding".
    pub onboarding_flag: &'static str,
    /// Key of the per-project map in that file, e.g. "projects".
    pub projects_key: &'static str,
    /// Booleans set to `true` under `<projects_key>[<container_workdir>]`, e.g.
    /// ["hasTrustDialogAccepted", "hasCompletedProjectOnboarding"]. Empty = no
    /// project entry is written.
    pub project_flags: &'static [&'static str],
}

/// All behavior that varies per coding agent. Plain `Copy` data (see §2 D1).
#[derive(Debug, Clone, Copy)]
pub struct CodingAgentProfile {
    pub kind: CodingAgentKind,
    /// Idle-detector tuning for sessions running this agent.
    pub idle: IdleTuning,
    /// Canonical argv tokens AC auto-injects to resume the agent's prior
    /// conversation, in argv order. Persistence stripping is provider-specific:
    /// Pi recipes are intentionally preserved because configured and injected
    /// `--continue` tokens have no persisted provenance.
    /// - Claude: `["--continue"]` (appended to argv)
    /// - Codex: `["resume", "--last"]` (prepended as a subcommand)
    /// - Antigravity: `["--continue"]` (prepended)
    /// - Pi: `["--continue"]` (inserted immediately after the executable)
    /// - Muse: `["resume", "--last"]` (appended to an EMPTY configured argv only;
    ///   any configured argument suppresses injection, #1873)
    pub resume_tokens: &'static [&'static str],
    /// #930 - host credential file this agent reuses in a container (None = no
    /// copy-in for this agent).
    pub container_credential: Option<ContainerCredentialSource>,
    /// Whether generated auto-self-clear instructions are supported for this
    /// provider. This capability is an absolute outer gate on user settings.
    pub auto_self_clear_supported: bool,
}

// #2156 - all five agents use `IdleTuning::AGENT_DEFAULT`: the pre-#260
// hard-coded idle constants plus the #2124 burst filter ON, so a recognised
// coding agent gets the filter regardless of what the persisted catalog
// contains. The per-profile `idle` field exists so a future agent can diverge
// (e.g. a longer `resize_grace` for a heavier TUI) without re-plumbing the
// detector.
const CLAUDE_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Claude,
    idle: IdleTuning::AGENT_DEFAULT,
    resume_tokens: &["--continue"],
    // #930 - verified end-to-end: host ~/.claude/.credentials.json copies to
    // <replica>/.claude/.credentials.json, read in-container as
    // /workspace/.claude/.credentials.json.
    container_credential: Some(ContainerCredentialSource {
        host_dir: ".claude",
        host_dir_env: Some("CLAUDE_CONFIG_DIR"),
        file: ".credentials.json",
        container_dir: ".claude",
        // #930 - without these, a valid copied token STILL shows the onboarding
        // wizard ("Select login method") and then the folder-trust dialog.
        // Reproduced in a real container; both prompts vanish once they are set.
        first_run: Some(ContainerFirstRunState {
            file: ".claude.json",
            onboarding_flag: "hasCompletedOnboarding",
            projects_key: "projects",
            project_flags: &["hasTrustDialogAccepted", "hasCompletedProjectOnboarding"],
        }),
    }),
    auto_self_clear_supported: true,
};
const CODEX_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Codex,
    // #2879 - the only profile that selects the Working-indicator detector.
    idle: IdleTuning {
        activity_detector: ActivityDetectorKind::CodexWorking,
        ..IdleTuning::AGENT_DEFAULT
    },
    resume_tokens: &["resume", "--last"],
    // #930 follow-up (needs CODEX_HOME container wiring verified, Q3):
    // Some(ContainerCredentialSource { host_dir: ".codex", host_dir_env: Some("CODEX_HOME"),
    //     file: "auth.json", container_dir: ".codex" })
    container_credential: None,
    auto_self_clear_supported: true,
};
const ANTIGRAVITY_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Antigravity,
    idle: IdleTuning::AGENT_DEFAULT,
    resume_tokens: &["--continue"],
    // #930 - no established container credential-file flow for Antigravity.
    container_credential: None,
    auto_self_clear_supported: true,
};
const PI_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Pi,
    idle: IdleTuning::AGENT_DEFAULT,
    resume_tokens: &["--continue"],
    container_credential: None,
    auto_self_clear_supported: false,
};
// #1873 - Muse Code. Workspace-latest resume (`muse resume --last`) is injected
// only by the trusted empty-argv local macOS/Linux seam in `commands::session`;
// no container credential flow, no auto-self-clear, no logical PTY submission.
const MUSE_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Muse,
    idle: IdleTuning::AGENT_DEFAULT,
    resume_tokens: &["resume", "--last"],
    container_credential: None,
    auto_self_clear_supported: false,
};

/// Idle-detector tuning for a session, given its (optional) agent kind.
/// `None` (plain shell / unrecognised agent) → `IdleTuning::DEFAULT`.
pub fn idle_tuning_for(kind: Option<CodingAgentKind>) -> IdleTuning {
    match kind {
        Some(k) => k.profile().idle,
        None => IdleTuning::DEFAULT,
    }
}

/// #2124 - the per-kind tuning plus the session's catalog `idleBurst` filter,
/// as raw primitives (`maxBytes`, `maxSecs`, `priorSilenceSecs`). `None` (no
/// catalog object) means NO OVERRIDE and equals `idle_tuning_for`, i.e. #2156
/// leaves the filter ON for a recognised agent and OFF for a plain shell.
/// Absent, non-finite or negative subfields take the documented defaults;
/// `maxSecs` is capped at 600 s and `priorSilenceSecs` at 86400 s so
/// `Duration::from_secs_f64` can never panic. `0` for `maxBytes` or `maxSecs`
/// disables the filter. Takes primitives on purpose: this module must not
/// depend on `config`.
pub fn idle_tuning_for_launch(
    kind: Option<CodingAgentKind>,
    burst: Option<(Option<u64>, Option<f64>, Option<f64>)>,
) -> IdleTuning {
    let mut tuning = idle_tuning_for(kind);
    let Some((max_bytes, max_secs, prior_silence_secs)) = burst else {
        return tuning;
    };
    tuning.burst_max_bytes = max_bytes.unwrap_or(IDLE_BURST_DEFAULT_MAX_BYTES);
    tuning.burst_window = Duration::from_secs_f64(
        sanitized_burst_secs(max_secs, IDLE_BURST_DEFAULT_MAX_SECS).min(IDLE_BURST_MAX_WINDOW_SECS),
    );
    tuning.burst_prior_silence = Duration::from_secs_f64(
        sanitized_burst_secs(prior_silence_secs, IDLE_BURST_DEFAULT_PRIOR_SILENCE_SECS)
            .min(IDLE_BURST_MAX_PRIOR_SILENCE_SECS),
    );
    tuning
}

/// A finite, non-negative subfield value, else `default`.
fn sanitized_burst_secs(value: Option<f64>, default: f64) -> f64 {
    value
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests;
