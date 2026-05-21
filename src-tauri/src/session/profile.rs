//! Per-coding-agent profile — the single source of truth for behavior that
//! varies by coding agent (Claude Code, Codex CLI, Gemini CLI).
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
    Gemini,
}

impl CodingAgentKind {
    /// Detect the coding agent from a spawn command (`shell` + `args`).
    ///
    /// Scans the shell and every whitespace-split arg token, reduces each to
    /// its executable basename (file stem, lowercased), and matches by
    /// **prefix** with precedence **Claude > Codex > Gemini**. Prefix match
    /// (not exact) is deliberate: it catches wrapper executables such as
    /// `claude-mb`, `codex-foo`, `gemini-bar`.
    ///
    /// THIS IS THE detector. `create_session_inner` (which stamps
    /// `Session::agent_kind`) and `strip_auto_injected_args` both call it, so
    /// the persisted recipe and the runtime identity can never disagree.
    pub fn detect(shell: &str, args: &[String]) -> Option<CodingAgentKind> {
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
        // Precedence claude > codex > gemini, scanning every token.
        if basenames.iter().any(|b| b.starts_with("claude")) {
            Some(CodingAgentKind::Claude)
        } else if basenames.iter().any(|b| b.starts_with("codex")) {
            Some(CodingAgentKind::Codex)
        } else if basenames.iter().any(|b| b.starts_with("gemini")) {
            Some(CodingAgentKind::Gemini)
        } else {
            None
        }
    }

    /// Resolve the full behavior profile for this kind.
    pub const fn profile(self) -> CodingAgentProfile {
        match self {
            CodingAgentKind::Claude => CLAUDE_PROFILE,
            CodingAgentKind::Codex => CODEX_PROFILE,
            CodingAgentKind::Gemini => GEMINI_PROFILE,
        }
    }
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
}

impl IdleTuning {
    /// Tuning for a plain shell / unrecognised agent. Also the per-field
    /// fallback when a session id is missing from the detector's tuning map.
    /// Values are identical to the pre-#260 `idle_detector.rs` constants.
    pub const DEFAULT: IdleTuning = IdleTuning {
        idle_threshold: Duration::from_millis(2500),
        resize_grace: Duration::from_millis(3000),
        seed_initial_activity: true,
    };
}

/// All behavior that varies per coding agent. Plain `Copy` data (see §2 D1).
#[derive(Debug, Clone, Copy)]
pub struct CodingAgentProfile {
    pub kind: CodingAgentKind,
    /// Idle-detector tuning for sessions running this agent.
    pub idle: IdleTuning,
    /// Argv tokens AC auto-injects to resume the agent's prior conversation,
    /// in argv order. The single source of truth for both injection
    /// (`create_session_inner`) and stripping (`strip_auto_injected_args`):
    /// - Claude → `["--continue"]` (appended to argv)
    /// - Codex → `["resume", "--last"]` (prepended as a subcommand)
    /// - Gemini → `["--resume", "latest"]` (prepended; the joined
    ///   `--resume=latest` form is handled by the Gemini stripper as a
    ///   recognised variant)
    pub resume_tokens: &'static [&'static str],
}

// All three agents currently use `IdleTuning::DEFAULT` — identical to the
// pre-#260 hard-coded constants, which GUARANTEES zero behavior change. The
// per-profile `idle` field exists so a future agent can diverge (e.g. a
// longer `resize_grace` for a heavier TUI) without re-plumbing the detector.
const CLAUDE_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Claude,
    idle: IdleTuning::DEFAULT,
    resume_tokens: &["--continue"],
};
const CODEX_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Codex,
    idle: IdleTuning::DEFAULT,
    resume_tokens: &["resume", "--last"],
};
const GEMINI_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Gemini,
    idle: IdleTuning::DEFAULT,
    resume_tokens: &["--resume", "latest"],
};

/// Idle-detector tuning for a session, given its (optional) agent kind.
/// `None` (plain shell / unrecognised agent) → `IdleTuning::DEFAULT`.
pub fn idle_tuning_for(kind: Option<CodingAgentKind>) -> IdleTuning {
    match kind {
        Some(k) => k.profile().idle,
        None => IdleTuning::DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_direct_claude_and_wrapper_basename() {
        assert_eq!(
            CodingAgentKind::detect("claude", &[]),
            Some(CodingAgentKind::Claude)
        );
        // claude-mb wrapper — prefix match.
        assert_eq!(
            CodingAgentKind::detect("claude-mb", &["--effort".into(), "max".into()]),
            Some(CodingAgentKind::Claude)
        );
    }

    #[test]
    fn detect_codex_and_gemini_direct() {
        assert_eq!(
            CodingAgentKind::detect("codex", &[]),
            Some(CodingAgentKind::Codex)
        );
        assert_eq!(
            CodingAgentKind::detect("gemini", &["-m".into(), "gpt-5".into()]),
            Some(CodingAgentKind::Gemini)
        );
    }

    #[test]
    fn detect_inside_cmd_wrapper_tokenized_and_embedded() {
        // cmd /C codex ...
        assert_eq!(
            CodingAgentKind::detect("cmd.exe", &["/C".into(), "codex".into()]),
            Some(CodingAgentKind::Codex)
        );
        // cmd /K "git pull && gemini --resume latest"  (embedded in one arg)
        assert_eq!(
            CodingAgentKind::detect(
                "cmd.exe",
                &["/K".into(), "git pull && gemini --resume latest".into()]
            ),
            Some(CodingAgentKind::Gemini)
        );
    }

    #[test]
    fn detect_precedence_claude_wins() {
        // A compound command mentioning both — Claude takes precedence,
        // matching create_session_inner's pre-#260 ordering.
        assert_eq!(
            CodingAgentKind::detect("cmd.exe", &["/K".into(), "codex && claude".into()]),
            Some(CodingAgentKind::Claude)
        );
    }

    #[test]
    fn detect_plain_shell_is_none() {
        assert_eq!(CodingAgentKind::detect("powershell.exe", &["-NoLogo".into()]), None);
        assert_eq!(CodingAgentKind::detect("cmd.exe", &[]), None);
    }

    #[test]
    fn detect_strips_known_exe_extension() {
        // `file_stem` drops the `.exe` suffix the basename match relies on
        // (dev-rust R1.5).
        assert_eq!(
            CodingAgentKind::detect("claude.exe", &[]),
            Some(CodingAgentKind::Claude)
        );
    }

    #[test]
    fn detect_space_in_shell_path_treats_shell_as_one_token() {
        // #260 G3 — `detect` treats `shell` as a SINGLE token; it does NOT
        // whitespace-split it the way pre-#260 `create_session_inner` split
        // the joined command string. A space-containing shell path whose real
        // executable is not an agent therefore resolves to `None` (the more
        // correct result — the executable here is `runner.exe`).
        assert_eq!(
            CodingAgentKind::detect("C:\\codex tools\\runner.exe", &[]),
            None
        );
    }

    #[test]
    fn idle_tuning_for_none_is_default() {
        assert_eq!(idle_tuning_for(None), IdleTuning::DEFAULT);
    }

    #[test]
    fn every_profile_seeds_initial_activity() {
        // The #260 fix must be on for all agents (and the default).
        for kind in [
            CodingAgentKind::Claude,
            CodingAgentKind::Codex,
            CodingAgentKind::Gemini,
        ] {
            assert!(kind.profile().idle.seed_initial_activity);
        }
        // Routed through the non-const `idle_tuning_for` so this stays a
        // runtime assertion (a bare `IdleTuning::DEFAULT.…` is const-folded,
        // which clippy::assertions_on_constants rejects).
        assert!(idle_tuning_for(None).seed_initial_activity);
    }

    /// dev-rust R1.5 — locks the §11 "all three profiles == DEFAULT → zero
    /// idle-tuning regression" guarantee. A future stray per-agent retune
    /// fails here loudly instead of silently shifting behavior.
    #[test]
    fn every_profile_uses_default_idle_tuning() {
        for kind in [
            CodingAgentKind::Claude,
            CodingAgentKind::Codex,
            CodingAgentKind::Gemini,
        ] {
            assert_eq!(kind.profile().idle, IdleTuning::DEFAULT);
        }
    }
}
