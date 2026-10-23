use super::*;
use crate::pty::inject::validate_pty_input_text;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

mod engine;
mod n_cases;
mod provisioning;

// ---- fixtures ---------------------------------------------------------

/// #1614 AC7.12 / D17, recognizer family 5. `injected-messages.toml` is a
/// USER-OWNED file: an entry the user has not edited is refreshed
/// automatically when a new default ships, and reconciliation decides that
/// by hashing the user's `template` against `known_default_sha256`.
///
/// This change touches exactly ONE line in this module, the `%WORKGROUP%`
/// DESCRIPTION inside the doc comment, and that line is never hashed. So:
/// the hash list must still be a SINGLE entry (a second hash appended here
/// would mean someone moved the template's bytes), the template must still
/// be 125 bytes, and `%WORKGROUP%` must still expand -- it appears in every
/// user-edited template on disk and renaming it would silently stop
/// expansion in each one.
#[test]
fn injected_messages_recognizer_is_untouched() {
    let spec = KNOWN_MESSAGES
        .iter()
        .find(|s| s.id == CONTEXT_ALERT_MESSAGE_ID)
        .expect("context-alert spec");

    assert_eq!(
        spec.known_default_sha256,
        &["e672581d47e7e4a4749b510f23eff72982ff3fa5261109122b3bdf8fdfda153f"],
        "known_default_sha256 must stay exactly one entry; a second hash here means the \
         shipped default's bytes moved and every pristine entry stops auto-refreshing"
    );
    assert_eq!(
        DEFAULT_CONTEXT_ALERT_TEMPLATE.len(),
        125,
        "the shipped default template must not be resized by this rename"
    );
    assert!(DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_WORKGROUP));
    assert_eq!(TOKEN_WORKGROUP, "%WORKGROUP%");

    let (out, missing) = expand_tokens(
        "member=%MEMBER% entity=%WORKGROUP%",
        &[
            (TOKEN_MEMBER, "dev-rust"),
            (TOKEN_WORKGROUP, "room-2-dev-team"),
        ],
    );
    assert_eq!(out, "member=dev-rust entity=room-2-dev-team");
    assert!(missing.is_empty(), "{missing:?}");

    // The one Rule R edit: the token's human DESCRIPTION says Room, while
    // the token itself is untouched.
    assert!(CONTEXT_ALERT_DOC_COMMENT.contains("%WORKGROUP%   room name"));
    assert!(!CONTEXT_ALERT_DOC_COMMENT.contains("workgroup name"));
}

const PINNED_SHA256: &str = "e672581d47e7e4a4749b510f23eff72982ff3fa5261109122b3bdf8fdfda153f";

/// A hypothetical next shipped default, with the v1 default recorded as a
/// previously shipped one. This is exactly the shape of a real upgrade and
/// is what the refresh, recognizer and BOM tests provision against.
const NEXT_CONTEXT_ALERT_TEMPLATE: &str =
    "[AC context alert] `%MEMBER%` in `%WORKGROUP%` crossed %THRESHOLDS%.";

const EXTRA_MESSAGE_ID: &str = "test-extra-message";
const EXTRA_TEMPLATE: &str = "extra `%MEMBER%` body";

/// The exact seeded bytes, transcribed from the plan rather than produced
/// by the code under test. Comparing the written file against
/// `canonical_seed_bytes` cannot fail, so an edit to a header constant
/// would drift from the specification with nothing going red.
const EXPECTED_SEED: &str = concat!(
    r##"# AgentsCommander injected PTY message templates.
# Managed by AgentsCommander. Edit the `template` values freely.
#
# Markdown is preserved byte for byte and passed through to the receiving
# agent as raw text; AgentsCommander does not render it. Use ''' literal
# strings so backticks, asterisks and line breaks survive unchanged.
#
# Delete a `template` value, an entry, or this whole file to fall back to the
# built-in default. An entry you have not edited is refreshed automatically
# when a new AgentsCommander version ships a better default; an entry you HAVE
# edited is never overwritten.
#
# Placeholders are %UPPERCASE% words between percent signs. There is no escape:
# a literal %MEMBER% cannot survive expansion. A lone % is ordinary text, so
# "100%" is safe. AgentsCommander's path placeholders (%AC_REPLICA_ROOT% and
# friends) do NOT apply here.
#
# See injected-messages.default.toml for the current canonical set.
schema_version = 1
coverage_version = 2

[messages.context-alert]
# Injected into the orchestrator's terminal when a member crosses a configured
# context-usage threshold.
# Placeholders:
#   %MEMBER%      name of the observed member, e.g. dev-rust
#   %WORKGROUP%   room name, e.g. room-2-dev-team
#   %THRESHOLDS%  thresholds just crossed, already formatted, e.g. 50%, 75%
#   %OBSERVED%    observed context use, e.g. 91% (best-effort human signal)
template = '''
[AC context alert] `%MEMBER%` in `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No action "##,
    r##"taken; you decide any follow-up.
'''

[messages.ci-started]
# Injected into the orchestrator's terminal when CI starts running on a room repo's
# exact HEAD.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %SHA%     abbreviated commit SHA, 7 characters
#   %AT%      observation time, local with a numeric UTC offset
template = '''
[AgentsCommander] CI started on %REPO% %BRANCH% (commit %SHA%) at %AT%. Status notice only; no "##,
    r##"action is required.
'''

[messages.ci-finished]
# Injected into the orchestrator's terminal when CI stops running on a room repo's
# exact HEAD. It never says whether CI passed.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %SHA%     abbreviated commit SHA, 7 characters
#   %AT%      observation time, local with a numeric UTC offset
template = '''
[AgentsCommander] CI finished on %REPO% %BRANCH% (commit %SHA%) at %AT%. AgentsCommander does "##,
    r##"not track pass or fail: run 'gh run list --branch %BRANCH%' before acting on this.
'''

[messages.branch-stale]
# Injected into the orchestrator's terminal when a room repo's branch falls behind
# the repository's default branch.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %BASE%    comparison base, already remote-qualified, e.g. main on GitHub
#   %BEHIND%  commit count behind, e.g. 4
#   %AT%      observation time, local with a numeric UTC offset
template = '''
[AgentsCommander] %REPO% %BRANCH% is now %BEHIND% commits behind %BASE% as of %AT%. If CI runs "##,
    r##"on this branch, it is validating an out-of-date base.
'''

[messages.notice-blind-gap]
# Appended to a CI or branch-stale notice when AgentsCommander could not reach
# GitHub for longer than ten minutes before the notice.
# Placeholders:
#   %GAP%    how long GitHub was unreachable, e.g. 47 minutes
#   %SINCE%  time of the previous confirmed state, local with a numeric UTC offset
template = '''
 AgentsCommander could not reach GitHub for %GAP%; the previous confirmed state is from %SINCE%.
'''
"##
);

/// The exact reference companion, transcribed the same way and for the same
/// reason as [`EXPECTED_SEED`].
const EXPECTED_REFERENCE: &str = concat!(
    r##"# AgentsCommander injected PTY message templates - CANONICAL REFERENCE.
#
# GENERATED FILE, DO NOT EDIT. AgentsCommander rewrites it whenever the shipped
# defaults change, and never reads it back. It exists so you can see the current
# canonical set next to your own injected-messages.toml.
#
# To change what AgentsCommander injects, edit injected-messages.toml instead.
schema_version = 1
coverage_version = 2

[messages.context-alert]
# Supported placeholders: %MEMBER%, %WORKGROUP%, %THRESHOLDS%, %OBSERVED%
# Injected into the orchestrator's terminal when a member crosses a configured
# context-usage threshold.
# Placeholders:
#   %MEMBER%      name of the observed member, e.g. dev-rust
#   %WORKGROUP%   room name, e.g. room-2-dev-team
#   %THRESHOLDS%  thresholds just crossed, already formatted, e.g. 50%, 75%
#   %OBSERVED%    observed context use, e.g. 91% (best-effort human signal)
template = '''
[AC context alert] `%MEMBER%` in `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No action "##,
    r##"taken; you decide any follow-up.
'''

[messages.ci-started]
# Supported placeholders: %REPO%, %BRANCH%, %SHA%, %AT%
# Injected into the orchestrator's terminal when CI starts running on a room repo's
# exact HEAD.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %SHA%     abbreviated commit SHA, 7 characters
#   %AT%      observation time, local with a numeric UTC offset
template = '''
[AgentsCommander] CI started on %REPO% %BRANCH% (commit %SHA%) at %AT%. Status notice only; no "##,
    r##"action is required.
'''

[messages.ci-finished]
# Supported placeholders: %REPO%, %BRANCH%, %SHA%, %AT%
# Injected into the orchestrator's terminal when CI stops running on a room repo's
# exact HEAD. It never says whether CI passed.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %SHA%     abbreviated commit SHA, 7 characters
#   %AT%      observation time, local with a numeric UTC offset
template = '''
[AgentsCommander] CI finished on %REPO% %BRANCH% (commit %SHA%) at %AT%. AgentsCommander does "##,
    r##"not track pass or fail: run 'gh run list --branch %BRANCH%' before acting on this.
'''

[messages.branch-stale]
# Supported placeholders: %REPO%, %BRANCH%, %BASE%, %BEHIND%, %AT%
# Injected into the orchestrator's terminal when a room repo's branch falls behind
# the repository's default branch.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %BASE%    comparison base, already remote-qualified, e.g. main on GitHub
#   %BEHIND%  commit count behind, e.g. 4
#   %AT%      observation time, local with a numeric UTC offset
template = '''
[AgentsCommander] %REPO% %BRANCH% is now %BEHIND% commits behind %BASE% as of %AT%. If CI runs "##,
    r##"on this branch, it is validating an out-of-date base.
'''

[messages.notice-blind-gap]
# Supported placeholders: %GAP%, %SINCE%
# Appended to a CI or branch-stale notice when AgentsCommander could not reach
# GitHub for longer than ten minutes before the notice.
# Placeholders:
#   %GAP%    how long GitHub was unreachable, e.g. 47 minutes
#   %SINCE%  time of the previous confirmed state, local with a numeric UTC offset
template = '''
 AgentsCommander could not reach GitHub for %GAP%; the previous confirmed state is from %SINCE%.
'''
"##
);

fn next_specs() -> [MessageSpec; 1] {
    [MessageSpec {
        id: CONTEXT_ALERT_MESSAGE_ID,
        default_template: NEXT_CONTEXT_ALERT_TEMPLATE,
        known_default_sha256: &[PINNED_SHA256],
        tokens: &[
            TOKEN_MEMBER,
            TOKEN_WORKGROUP,
            TOKEN_THRESHOLDS,
            TOKEN_OBSERVED,
        ],
        doc_comment: CONTEXT_ALERT_DOC_COMMENT,
    }]
}

/// sha256 of `NEXT_CONTEXT_ALERT_TEMPLATE`. Pinned because
/// `known_default_sha256` is `&'static [&'static str]`; the test that uses
/// it recomputes the value so the pin cannot drift.
const NEXT_SHA256: &str = "8bfd1f123875b2b1397695a77086ed1361f0351e6d77c9d342290ae7be6a813e";

/// A third shipped default, with BOTH earlier ones recorded. N16 needs it:
/// a second pass that reuses `next_specs` lands in `AlreadyCurrent`, which
/// passes even with no recognizer at all, so it would assert a capability
/// it never exercises.
const THIRD_CONTEXT_ALERT_TEMPLATE: &str =
    "[AC context alert] `%MEMBER%` in `%WORKGROUP%` is at %THRESHOLDS%.";

fn third_specs() -> [MessageSpec; 1] {
    [MessageSpec {
        id: CONTEXT_ALERT_MESSAGE_ID,
        default_template: THIRD_CONTEXT_ALERT_TEMPLATE,
        known_default_sha256: &[PINNED_SHA256, NEXT_SHA256],
        tokens: &[
            TOKEN_MEMBER,
            TOKEN_WORKGROUP,
            TOKEN_THRESHOLDS,
            TOKEN_OBSERVED,
        ],
        doc_comment: CONTEXT_ALERT_DOC_COMMENT,
    }]
}

fn two_specs() -> [MessageSpec; 2] {
    [
        MessageSpec {
            id: CONTEXT_ALERT_MESSAGE_ID,
            default_template: DEFAULT_CONTEXT_ALERT_TEMPLATE,
            known_default_sha256: &[PINNED_SHA256],
            tokens: &[
                TOKEN_MEMBER,
                TOKEN_WORKGROUP,
                TOKEN_THRESHOLDS,
                TOKEN_OBSERVED,
            ],
            doc_comment: CONTEXT_ALERT_DOC_COMMENT,
        },
        MessageSpec {
            id: EXTRA_MESSAGE_ID,
            default_template: EXTRA_TEMPLATE,
            known_default_sha256: &[],
            tokens: &[TOKEN_MEMBER],
            doc_comment: "# a second shipped message",
        },
    ]
}

fn alert_values() -> [(&'static str, &'static str); 4] {
    [
        (TOKEN_MEMBER, "dev-rust"),
        (TOKEN_WORKGROUP, "wg-2-dev-team"),
        (TOKEN_THRESHOLDS, "50%, 75%, 90%"),
        (TOKEN_OBSERVED, "91%"),
    ]
}

/// Rule 1 of section 9.3: every filesystem test gets its own temp dir and
/// no test ever calls `config_dir()`, which under `cargo test` resolves to
/// a real directory next to the test binary.
fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read")
}

fn main_path(dir: &Path) -> PathBuf {
    dir.join(INJECTED_MESSAGES_FILENAME)
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join(INJECTED_MESSAGES_STATE_FILENAME)
}

fn reference_path(dir: &Path) -> PathBuf {
    dir.join(INJECTED_MESSAGES_REFERENCE_FILENAME)
}

// ---- write counter (N14) ---------------------------------------------

static WRITE_LOG: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static COUNTING: AtomicBool = AtomicBool::new(false);
/// Held for the whole of any test that counts writes, so a concurrently
/// running test cannot pollute the log.
static COUNTING_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn record_write(path: &Path) {
    if !COUNTING.load(Ordering::SeqCst) {
        return;
    }
    if let Ok(mut log) = WRITE_LOG.lock() {
        log.push(path.to_path_buf());
    }
}

fn start_counting() {
    if let Ok(mut log) = WRITE_LOG.lock() {
        log.clear();
    }
    COUNTING.store(true, Ordering::SeqCst);
}

/// Only writes under `dir`, so tests running in parallel in the same
/// process cannot pollute the count.
fn stop_counting(dir: &Path) -> Vec<PathBuf> {
    COUNTING.store(false, Ordering::SeqCst);
    WRITE_LOG
        .lock()
        .map(|log| {
            log.iter()
                .filter(|path| path.starts_with(dir))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The `coverage_version` [`newer_coverage_version_keeps_rendering_from_file`]
/// writes into its fixture. It MUST stay strictly above `COVERAGE_VERSION`:
/// the writer block is gated on a strict `>` in `analyze_source`, so an equal
/// version leaves `writer_disabled` at `None` and the fixture stops
/// exercising the downgrade branch. Naming it here is what makes that
/// invariant assertable instead of a literal nobody re-checks.
const NEWER_COVERAGE_VERSION: u32 = 3;

/// The `[messages.<id>]` headers of an appended tail, in file order. Both
/// append tests assert against [`expected_appended_headers`] rather than a
/// count, so a wrong id or a wrong order is caught, not merely a wrong size.
fn appended_headers(tail: &str) -> Vec<String> {
    tail.lines()
        .filter(|line| line.starts_with("[messages."))
        .map(|line| line.to_string())
        .collect()
}

fn expected_appended_headers() -> Vec<String> {
    KNOWN_MESSAGES
        .iter()
        .skip(1)
        .map(|spec| format!("[messages.{}]", spec.id))
        .collect()
}
