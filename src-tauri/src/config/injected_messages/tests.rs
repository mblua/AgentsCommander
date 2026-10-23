use super::*;
use crate::pty::inject::validate_pty_input_text;
use std::sync::atomic::{AtomicBool, Ordering};

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

// ---- 1..7: the pure engine -------------------------------------------

#[test]
fn default_context_alert_template_is_pinned() {
    assert_eq!(
        DEFAULT_CONTEXT_ALERT_TEMPLATE,
        "[AC context alert] `%MEMBER%` in `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No \
         action taken; you decide any follow-up."
    );
    assert_eq!(DEFAULT_CONTEXT_ALERT_TEMPLATE.chars().count(), 125);
    assert_eq!(DEFAULT_CONTEXT_ALERT_TEMPLATE.len(), 125);
    assert_eq!(
        sha256_hex(DEFAULT_CONTEXT_ALERT_TEMPLATE.as_bytes()),
        PINNED_SHA256
    );
    assert!(DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_MEMBER));
    assert!(DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_WORKGROUP));
    assert!(DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_THRESHOLDS));
    assert!(!DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_OBSERVED));
    // The shipped-default list must always recognize what it ships.
    assert!(KNOWN_MESSAGES[0]
        .known_default_sha256
        .contains(&PINNED_SHA256));
}

#[test]
fn template_override_renders_user_text() {
    let template = "## %MEMBER%\n\n- **%WORKGROUP%** at `%OBSERVED%`\n- crossed %THRESHOLDS%";
    assert_eq!(
        render_with_template(template, &alert_values()),
        "## dev-rust\n\n- **wg-2-dev-team** at `91%`\n- crossed 50%, 75%, 90%"
    );
}

#[test]
fn markdown_and_multiline_survive_round_trip() {
    let template = "# Heading\n\n**bold** and `code` and *stars*\n\n- item one\n- item \
        two\n\n```rust\nfn main() {}\n```\n\n> quote %MEMBER%\n\n| a | b |\n|---|---|\n| 1 | 2 |";
    let expected = template.replace(TOKEN_MEMBER, "dev-rust");
    assert_eq!(render_with_template(template, &alert_values()), expected);
}

#[test]
fn unknown_placeholder_is_left_literal() {
    let rendered = render_with_template("%FOO% %MEMBER% %BAR_2% 100% done", &alert_values());
    assert_eq!(rendered, "%FOO% dev-rust %BAR_2% 100% done");
    assert!(!rendered.is_empty());
    // The unknown-token grammar must not fire on ordinary prose full of
    // bare percent signs, which is what rendered thresholds look like.
    let (_, unknown) = expand_tokens("50%, 75%, 90% and 100% del contexto", &alert_values());
    assert!(
        unknown.is_empty(),
        "unexpected unknown tokens: {:?}",
        unknown
    );
    // Case-insensitive recognition, for the warning only: `%member%` is not
    // substituted, but it is reported rather than silently ignored.
    let (out, unknown) = expand_tokens("%member%", &alert_values());
    assert_eq!(out, "%member%");
    assert_eq!(unknown, vec!["%member%".to_string()]);
}

#[test]
fn expansion_is_single_pass() {
    let values = [(TOKEN_MEMBER, "dev-%WORKGROUP%"), (TOKEN_WORKGROUP, "wg-2")];
    let (out, _) = expand_tokens("m=%MEMBER% w=%WORKGROUP%", &values);
    assert_eq!(out, "m=dev-%WORKGROUP% w=wg-2");
    assert_ne!(out, "m=dev-wg-2 w=wg-2");
    // A substituted value is never rescanned, so an adjacent token cannot
    // be manufactured out of one.
    let (out, _) = expand_tokens("%THRESHOLDS%OBSERVED%", &alert_values());
    assert_eq!(out, "50%, 75%, 90%OBSERVED%");
}

#[test]
fn sanitize_strips_c0_c1_and_del() {
    let hostile = "a\u{1b}[31mred\u{1b}]0;title\u{7}b\rc\u{7f}d\u{85}e\n\tkeep `md` *stars*";
    let out = sanitize(hostile);
    assert_eq!(out, "a[31mred]0;titlebcde\n\tkeep `md` *stars*");
    assert!(!out.contains('\u{1b}'));
    assert!(!out.contains('\r'));
    assert!(!out.contains('\u{7f}'));
    assert!(!out.contains('\u{85}'));
    assert!(out.contains('\n') && out.contains('\t'));
}

#[test]
fn sanitize_strips_bidi_and_separator_class() {
    let hostile =
        "a\u{202e}b\u{2066}c\u{2069}d\u{2028}e\u{200f}f\u{61c}g\u{200e}h\u{2029}i\u{202a}j";
    let out = sanitize(hostile);
    assert_eq!(out, "abcdefghij");
    for forbidden in [
        '\u{202e}', '\u{2066}', '\u{2069}', '\u{2028}', '\u{200f}', '\u{61c}', '\u{200e}',
        '\u{2029}', '\u{202a}',
    ] {
        assert!(!out.contains(forbidden), "kept U+{:04X}", forbidden as u32);
    }
}

#[test]
fn sanitize_neutralizes_response_marker_prefix() {
    let out = sanitize("x %%AC_RESPONSE::abc::START%% y %%AC_RESPONSE::abc::END%% z");
    assert_eq!(
        out,
        "x %AC_RESPONSE::abc::START%% y %AC_RESPONSE::abc::END%% z"
    );
    assert!(!out.contains("%%AC_RESPONSE::"));

    // The two literals `scan_response_markers` reconstructs per in-flight
    // rid. Neither may survive, for any run length.
    let start_marker = format!("%%AC_RESPONSE::{}::START%%", "abc");
    let end_marker = format!("%%AC_RESPONSE::{}::END%%", "abc");
    assert!(!out.contains(&start_marker));
    assert!(!out.contains(&end_marker));

    // Runs of two, three and four. A single `replace` turns the run of
    // three back into a match, which is exactly why the collapse has to be
    // a fixed point rather than one pass of substitution.
    for run in 2..=4 {
        let hostile = format!("{}AC_RESPONSE::abc::START%%", "%".repeat(run));
        let out = sanitize(&hostile);
        assert_eq!(out, "%AC_RESPONSE::abc::START%%", "run of {}", run);
        assert!(!out.contains("%%AC_RESPONSE::"), "run of {}", run);
        assert!(!out.contains(&start_marker), "run of {}", run);
        assert_eq!(sanitize(&out), out, "not idempotent for run of {}", run);
    }

    // Expansion runs before sanitization, so `%OBSERVED%` hands a third `%`
    // to an operator who only typed two.
    let expanded = render_with_template("%OBSERVED%%%AC_RESPONSE::abc::START%%", &alert_values());
    assert_eq!(expanded, "91%AC_RESPONSE::abc::START%%");
    assert!(!expanded.contains("%%AC_RESPONSE::"));
    assert!(!expanded.contains(&start_marker));
    assert_eq!(sanitize(&expanded), expanded);

    // Runs of length 0 and 1 are left exactly as they are.
    assert_eq!(sanitize("AC_RESPONSE::x"), "AC_RESPONSE::x");
    assert_eq!(sanitize("%AC_RESPONSE::x"), "%AC_RESPONSE::x");

    // The step order is load-bearing in one direction: step 3 must stay
    // AFTER step 2, because step 2 can bring two `%` together that a
    // stripped scalar separated. Nothing else in the matrix distinguishes
    // the two orders, so a refactor that swaps them would leave the whole
    // suite green while reopening the hole this collapse closes.
    assert!(!sanitize("%\u{202e}%AC_RESPONSE::abc::START%%").contains("%%AC_RESPONSE::"));
    assert_eq!(
        sanitize("%\u{202e}%AC_RESPONSE::abc::START%%"),
        "%AC_RESPONSE::abc::START%%"
    );

    // Idempotence over the whole set, including adjacent occurrences, where
    // the backward walk of one must not eat the other's text.
    for probe in [
        "x %%AC_RESPONSE::abc::START%% y %%AC_RESPONSE::abc::END%% z",
        "%%%%AC_RESPONSE::",
        "AC_RESPONSE::%%AC_RESPONSE::",
        "%%AC_RESPONSE::%%%AC_RESPONSE::",
        "no marker here",
        "",
    ] {
        let once = sanitize(probe);
        assert_eq!(sanitize(&once), once, "not idempotent: {:?}", probe);
        assert!(!once.contains("%%AC_RESPONSE::"), "survived: {:?}", probe);
    }
}

#[test]
fn render_truncates_at_cap_on_char_boundary() {
    let template = "🚨".repeat(4096);
    let out = render_with_template(&template, &alert_values());
    assert!(out.len() <= MAX_RENDERED_BYTES);
    // A 4-byte scalar never straddles the cap: 4096 is divisible by 4 here,
    // but the invariant is that the result is valid UTF-8 either way.
    assert_eq!(out.chars().count(), out.len() / 4);
    assert!(std::str::from_utf8(out.as_bytes()).is_ok());

    let odd = format!("{}{}", "a".repeat(MAX_RENDERED_BYTES - 1), "🚨");
    let out = render_with_template(&odd, &alert_values());
    assert!(out.len() <= MAX_RENDERED_BYTES);
    assert_eq!(out.len(), MAX_RENDERED_BYTES - 1);
}

// ---- 8..21: provisioning ---------------------------------------------

#[test]
fn missing_file_seeds_canonical_bytes() {
    let dir = tempdir();
    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Seeded)
    );

    // Against the transcribed literal, never against the generator.
    assert_eq!(EXPECTED_SEED.len(), 3712, "the pinned seed is 3712 bytes");
    assert!(!EXPECTED_SEED.contains('\r'), "the pinned seed is LF");
    let written = read(&main_path(dir.path()));
    assert_eq!(written, EXPECTED_SEED);
    assert!(!written.contains("\r\n"), "seeded file must be LF");
    assert!(written.ends_with("'''\n"));
    assert!(!written.ends_with("\n\n"), "exactly one trailing newline");
    assert!(written.contains("schema_version = 1"));
    assert!(written.contains("coverage_version = 2"));
    assert!(written.contains("[messages.context-alert]"));

    let state: InjectedMessagesState =
        serde_json::from_str(&read(&state_path(dir.path()))).expect("sidecar");
    assert_eq!(state.schema_version, SCHEMA_VERSION);
    assert_eq!(
        state.messages[CONTEXT_ALERT_MESSAGE_ID].last_seeded_sha256,
        Some(PINNED_SHA256.to_string())
    );
    // camelCase on the wire.
    assert!(read(&state_path(dir.path())).contains("lastSeededSha256"));
}

#[test]
fn untouched_entry_is_refreshed_when_default_changes() {
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let before = read(&main_path(dir.path()));

    let specs = next_specs();
    let report = provision(dir.path(), &specs, None).expect("refresh");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );

    let after = read(&main_path(dir.path()));
    assert_ne!(before, after);
    assert!(after.contains(NEXT_CONTEXT_ALERT_TEMPLATE));
    assert!(!after.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert_eq!(
        load_registry_from_dir(dir.path()).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&NEXT_CONTEXT_ALERT_TEMPLATE.to_string())
    );

    let state: InjectedMessagesState =
        serde_json::from_str(&read(&state_path(dir.path()))).expect("sidecar");
    assert_eq!(
        state.messages[CONTEXT_ALERT_MESSAGE_ID].last_seeded_sha256,
        Some(sha256_hex(NEXT_CONTEXT_ALERT_TEMPLATE.as_bytes()))
    );

    // And it settles: a second run with the same specs changes nothing.
    let report = provision(dir.path(), &specs, None).expect("settle");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );
    assert_eq!(read(&main_path(dir.path())), after);
}

#[test]
fn edited_entry_is_never_clobbered() {
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let edited = read(&main_path(dir.path()))
        .replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "MY OWN WORDING %MEMBER%");
    std::fs::write(main_path(dir.path()), &edited).expect("edit");

    let specs = next_specs();
    for _ in 0..2 {
        let report = provision(dir.path(), &specs, None).expect("provision");
        assert_eq!(report.writer_disabled, None);
        assert_eq!(
            report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
            Some(&EntryOutcome::PreservedUserEdit)
        );
        assert_eq!(read(&main_path(dir.path())), edited);
    }
    assert_eq!(
        load_registry_from_dir(dir.path()).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"MY OWN WORDING %MEMBER%".to_string())
    );
    let state: InjectedMessagesState =
        serde_json::from_str(&read(&state_path(dir.path()))).expect("sidecar");
    assert_eq!(
        state.messages[CONTEXT_ALERT_MESSAGE_ID].last_observed_sha256,
        Some(sha256_hex(b"MY OWN WORDING %MEMBER%"))
    );
}

#[test]
fn surgical_write_preserves_comments_and_unknown_keys() {
    let dir = tempdir();
    let source = format!(
        "# lead comment\nunknown_top = 7\nschema_version = 1\n\n[messages.zzz-other]\ntemplate = \
         '''\nzzz\n'''\n\n[messages.context-alert]\n# doc comment\nunknown_inside = \
         true\ntemplate = '''\n{}\n'''\n# trailing comment\n",
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    // Reconstruct the expectation from the span itself, which is exactly
    // what the writer does. No absolute offset is asserted.
    let doc: WriteDoc = toml::from_str(&source).expect("write parse");
    let span = doc.messages[CONTEXT_ALERT_MESSAGE_ID]
        .template
        .as_ref()
        .expect("span")
        .span();
    assert!(source[span.clone()].starts_with("'''"));
    assert!(source[span.clone()].ends_with("'''"));
    let expected = format!(
        "{}{}{}",
        &source[..span.start],
        quote_literal(NEXT_CONTEXT_ALERT_TEMPLATE, false),
        &source[span.end..]
    );

    let specs = next_specs();
    let report = provision(dir.path(), &specs, None).expect("refresh");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(read(&main_path(dir.path())), expected);

    let after = read(&main_path(dir.path()));
    assert!(after.contains("# lead comment"));
    assert!(after.contains("unknown_top = 7"));
    assert!(after.contains("# doc comment"));
    assert!(after.contains("unknown_inside = true"));
    assert!(after.contains("# trailing comment"));
    assert!(after.contains("[messages.zzz-other]"));
    assert!(after.contains("zzz"));
}

#[test]
fn append_is_idempotent_and_preserves_existing_bytes() {
    let dir = tempdir();
    let source = format!(
        "# operator comment\nschema_version = 1\n\n[messages.context-alert]\ntemplate = \
         '''\n{}\n'''\n# a trailing comment with no newline after it",
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    let specs = two_specs();
    let report = provision(dir.path(), &specs, None).expect("append");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(EXTRA_MESSAGE_ID),
        Some(&EntryOutcome::Appended)
    );
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );

    let after = read(&main_path(dir.path()));
    // The original bytes are a prefix: nothing before the append moved, and
    // the trailing comment did not swallow the appended block.
    assert!(after.starts_with(&source));
    assert!(after.contains(&format!("[messages.{}]", EXTRA_MESSAGE_ID)));
    assert!(after.contains(EXTRA_TEMPLATE));

    let report = provision(dir.path(), &specs, None).expect("second run");
    assert_eq!(
        report.outcomes.get(EXTRA_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );
    assert_eq!(read(&main_path(dir.path())), after);
}

#[test]
fn malformed_toml_preserves_bytes_and_disables_writer() {
    let dir = tempdir();
    let source = "schema_version = 1\n[messages.context-alert\ntemplate = 'oops\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), source);
    assert!(load_registry_from_dir(dir.path()).is_empty());
    assert_eq!(
        render(CONTEXT_ALERT_MESSAGE_ID, &alert_values()),
        render_with_template(DEFAULT_CONTEXT_ALERT_TEMPLATE, &alert_values())
    );
}

#[test]
fn newer_schema_version_disables_writer_and_renders_defaults() {
    let dir = tempdir();
    let source =
        "schema_version = 2\ncoverage_version = 1\n\n[messages.context-alert]\ntemplate = \
        '''\nOPERATOR TEXT %MEMBER%\n'''\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), source);
    // Structure unknown: fall back to the built-in defaults.
    assert!(load_registry_from_dir(dir.path()).is_empty());
}

/// The `coverage_version` [`newer_coverage_version_keeps_rendering_from_file`]
/// writes into its fixture. It MUST stay strictly above `COVERAGE_VERSION`:
/// the writer block is gated on a strict `>` in `analyze_source`, so an equal
/// version leaves `writer_disabled` at `None` and the fixture stops
/// exercising the downgrade branch. Naming it here is what makes that
/// invariant assertable instead of a literal nobody re-checks.
const NEWER_COVERAGE_VERSION: u32 = 3;

#[test]
fn newer_coverage_version_keeps_rendering_from_file() {
    let dir = tempdir();
    let source = format!(
        "schema_version = 1\ncoverage_version = {}\n\n[messages.context-alert]\ntemplate = \
         '''\nOPERATOR TEXT %MEMBER%\n'''\n",
        NEWER_COVERAGE_VERSION
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), source);
    // A downgrade must not discard the operator's text.
    assert_eq!(
        load_registry_from_dir(dir.path()).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"OPERATOR TEXT %MEMBER%".to_string())
    );
}

#[test]
fn git_conflict_markers_disable_writer() {
    // Half one: markers at top level, which is also a TOML parse error.
    let dir = tempdir();
    let top_level =
        "schema_version = 1\n<<<<<<< HEAD\nmine = 1\n=======\ntheirs = 2\n>>>>>>> other\n";
    std::fs::write(main_path(dir.path()), top_level).expect("write");
    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), top_level);

    // Half two: markers inside a ''' literal, which parses cleanly and is
    // the case the scan exists to catch.
    let dir = tempdir();
    let inside = "schema_version = 1\n\n[messages.context-alert]\ntemplate = '''\n<<<<<<< \
        HEAD\nmine\n=======\ntheirs\n>>>>>>> other\n'''\n";
    std::fs::write(main_path(dir.path()), inside).expect("write");
    assert!(toml::from_str::<toml::Value>(inside).is_ok());
    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), inside);
    assert!(load_registry_from_dir(dir.path()).is_empty());
}

#[test]
fn unknown_message_id_and_unknown_key_are_preserved_not_fatal() {
    let dir = tempdir();
    let source =
        "schema_version = 1\nunknown_top = \"x\"\n\n[messages.not-a-known-id]\ntemplate = \
        '''\nsomething\n'''\n\n[messages.context-alert]\nunknown_inside = 1\ntemplate = \
        '''\nOPERATOR %MEMBER%\n'''\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert_eq!(report.writer_disabled, None);
    // The source carries no `coverage_version`, so the writer is enabled and
    // the four ids this binary knows and the file does not are appended. The
    // operator's bytes stay a prefix: nothing already written moves.
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with(source));
    let tail = &after[source.len()..];
    assert_eq!(
        appended_headers(tail),
        expected_appended_headers(),
        "the appended tail holds exactly the four new ids, in KNOWN_MESSAGES order"
    );
    assert_eq!(
        load_registry_from_dir(dir.path()).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"OPERATOR %MEMBER%".to_string())
    );
    for spec in KNOWN_MESSAGES.iter().skip(1) {
        assert_eq!(
            load_registry_from_dir(dir.path()).get(spec.id),
            Some(&spec.default_template.to_string()),
            "{}",
            spec.id
        );
    }
    // The unknown id is preserved but never rendered.
    assert!(!load_registry_from_dir(dir.path()).contains_key("not-a-known-id"));
}

#[test]
fn missing_or_corrupt_sidecar_uses_recognizer() {
    // Recognized shipped default, no sidecar at all: refreshed.
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    std::fs::remove_file(state_path(dir.path())).expect("drop sidecar");
    let specs = next_specs();
    let report = provision(dir.path(), &specs, None).expect("provision");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );
    assert!(read(&main_path(dir.path())).contains(NEXT_CONTEXT_ALERT_TEMPLATE));

    // Unrecognized content, corrupt sidecar: preserved.
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let edited =
        read(&main_path(dir.path())).replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "hand written");
    std::fs::write(main_path(dir.path()), &edited).expect("edit");
    std::fs::write(state_path(dir.path()), "{ not json").expect("corrupt");
    let report = provision(dir.path(), &specs, None).expect("provision");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit)
    );
    assert_eq!(read(&main_path(dir.path())), edited);
}

#[test]
fn empty_registry_renders_embedded_default() {
    // Rule 2 of section 9.3: under cfg(test) the process registry is empty.
    assert!(registry().is_empty());
    assert_eq!(
        render(CONTEXT_ALERT_MESSAGE_ID, &alert_values()),
        "[AC context alert] `dev-rust` in `wg-2-dev-team` reached threshold(s): 50%, 75%, 90%. No \
         action taken; you decide any follow-up."
    );
    // An id this binary does not know renders nothing at all.
    assert_eq!(render("no-such-message", &alert_values()), "");
}

#[test]
fn reference_companion_is_regenerated_and_overwritten() {
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let canonical = read(&reference_path(dir.path()));
    assert_eq!(canonical, canonical_reference_bytes(&KNOWN_MESSAGES));
    assert!(canonical.contains("DO NOT EDIT"));
    assert!(canonical.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert!(canonical.contains(TOKEN_OBSERVED));

    let main_before = read(&main_path(dir.path()));
    std::fs::write(reference_path(dir.path()), "hand edited\n").expect("edit companion");
    ensure_injected_messages(dir.path()).expect("regenerate");
    assert_eq!(read(&reference_path(dir.path())), canonical);
    assert_eq!(read(&main_path(dir.path())), main_before);
}

#[test]
fn reseed_id_and_all_reset_with_backup() {
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let seeded = read(&main_path(dir.path()));
    let edited = format!(
        "{}\n# an operator comment kept across reseed\nunknown_top = 3\n",
        seeded.replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "MY WORDING")
    );
    std::fs::write(main_path(dir.path()), &edited).expect("edit");

    let reset = reseed(
        dir.path(),
        ReseedTarget::Id(CONTEXT_ALERT_MESSAGE_ID.to_string()),
    )
    .expect("reseed --id");
    assert_eq!(reset, vec![CONTEXT_ALERT_MESSAGE_ID.to_string()]);
    let after = read(&main_path(dir.path()));
    assert!(after.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert!(!after.contains("MY WORDING"));
    assert!(after.contains("# an operator comment kept across reseed"));
    assert!(after.contains("unknown_top = 3"));

    // The backup is located by globbing, never by reconstructing the name.
    let backups: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .expect("read dir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".bak-"))
        })
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "expected exactly one backup: {:?}",
        backups
    );
    assert_eq!(read(&backups[0]), edited);

    let state: InjectedMessagesState =
        serde_json::from_str(&read(&state_path(dir.path()))).expect("sidecar");
    assert_eq!(
        state.messages[CONTEXT_ALERT_MESSAGE_ID].last_seeded_sha256,
        Some(PINNED_SHA256.to_string())
    );

    // --all goes through the same surgical writer.
    std::fs::write(
        main_path(dir.path()),
        after.replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "EDITED AGAIN"),
    )
    .expect("edit");
    let reset = reseed(dir.path(), ReseedTarget::All).expect("reseed --all");
    // Every covered id is reset, in the sorted order `reseed` reports.
    let mut all_ids: Vec<String> = KNOWN_MESSAGES
        .iter()
        .map(|spec| spec.id.to_string())
        .collect();
    all_ids.sort();
    assert_eq!(reset, all_ids);
    let after_all = read(&main_path(dir.path()));
    assert!(after_all.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert!(after_all.contains("# an operator comment kept across reseed"));
    assert!(after_all.contains("unknown_top = 3"));

    // An unknown id exits non-zero and changes nothing.
    let before = read(&main_path(dir.path()));
    let err = reseed(dir.path(), ReseedTarget::Id("no-such-id".to_string()))
        .expect_err("unknown id must fail");
    assert!(err.contains("no-such-id"));
    assert!(err.contains(CONTEXT_ALERT_MESSAGE_ID));
    assert_eq!(read(&main_path(dir.path())), before);
}

#[test]
fn seeded_file_round_trips_to_pinned_default_without_extra_newline() {
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let source = read(&main_path(dir.path()));

    let root: toml::Value = toml::from_str(&source).expect("parse");
    let raw = root["messages"][CONTEXT_ALERT_MESSAGE_ID]["template"]
        .as_str()
        .expect("string");
    // TOML trims the newline after the opening ''' but not the one before
    // the closing ''', so the raw value carries exactly one.
    assert!(raw.ends_with('\n'));
    assert_eq!(raw, format!("{}\n", DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert_eq!(
        normalize_parsed_template(raw),
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );

    // Without normalization the frame would be \n...\n\n\r.
    let rendered = render_with_template(&normalize_parsed_template(raw), &alert_values());
    assert_eq!(
        format!("\n{}\n\r", rendered),
        "\n[AC context alert] `dev-rust` in `wg-2-dev-team` reached threshold(s): 50%, 75%, 90%. \
         No action taken; you decide any follow-up.\n\r"
    );
}

// ---- N1..N16 ---------------------------------------------------------

#[test]
fn crlf_edit_is_not_a_content_edit() {
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let lf = read(&main_path(dir.path()));
    let crlf = lf.replace('\n', "\r\n");
    std::fs::write(main_path(dir.path()), &crlf).expect("save as CRLF");

    let lf_model = analyze_source(&lf);
    let crlf_model = analyze_source(&crlf);
    let lf_value = lf_model.templates[CONTEXT_ALERT_MESSAGE_ID].clone();
    let crlf_value = crlf_model.templates[CONTEXT_ALERT_MESSAGE_ID].clone();
    assert_eq!(lf_value, crlf_value);
    assert_eq!(
        sha256_hex(lf_value.as_bytes()),
        sha256_hex(crlf_value.as_bytes())
    );

    // A line-ending change must not be read as an edit, and a rewrite must
    // not create a mixed-ending file.
    let specs = next_specs();
    let report = provision(dir.path(), &specs, None).expect("provision");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );
    let after = read(&main_path(dir.path()));
    assert!(after.contains("'''\r\n"));
    assert_eq!(after.matches('\n').count(), after.matches("\r\n").count());
    assert_eq!(
        analyze_source(&after).templates[CONTEXT_ALERT_MESSAGE_ID],
        NEXT_CONTEXT_ALERT_TEMPLATE
    );
}

#[test]
fn entry_without_template_or_with_wrong_type_degrades_only_that_entry() {
    let dir = tempdir();
    let source = format!(
        "schema_version = 1\n\n[messages.context-alert]\n# the operator deleted the template \
         line\n\n[messages.{}]\ntemplate = 42\n\n[messages.healthy]\ntemplate = \
         '''\nHEALTHY\n'''\n",
        EXTRA_MESSAGE_ID
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    let model = analyze_source(&source);
    assert_eq!(model.strict_invalid, None);
    assert_eq!(
        model.entries.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryValue::Malformed)
    );
    assert_eq!(
        model.entries.get(EXTRA_MESSAGE_ID),
        Some(&EntryValue::Malformed)
    );
    assert_eq!(
        model.entries.get("healthy"),
        Some(&EntryValue::Template("HEALTHY".to_string()))
    );

    let specs = two_specs();
    let report = provision(dir.path(), &specs, None).expect("provision");
    assert_eq!(
        report.writer_disabled, None,
        "one bad entry must not disable the file"
    );
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit)
    );
    assert_eq!(read(&main_path(dir.path())), source);
    // A table-valued `template` reports a span over the whole header and
    // body, so "present but not a string" must never become a rewrite.
    let dir = tempdir();
    let table_valued =
        "schema_version = 1\n\n[messages.context-alert.template]\nk = 1\n".to_string();
    std::fs::write(main_path(dir.path()), &table_valued).expect("write");
    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert_eq!(report.writer_disabled, None);
    // The unreadable entry stays byte-identical and the four missing ids are
    // appended after it.
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with(&table_valued));
    assert_eq!(
        appended_headers(&after[table_valued.len()..]),
        expected_appended_headers()
    );
}

#[test]
fn every_shipped_default_is_literal_quotable() {
    for spec in &KNOWN_MESSAGES {
        assert!(
            is_literal_quotable(spec.default_template),
            "`{}` cannot be written as a TOML literal",
            spec.id
        );
    }
    assert!(!is_literal_quotable("has ''' inside"));
    assert!(!is_literal_quotable("has \r inside"));
    assert!(!is_literal_quotable("has \u{0}} inside"));
    assert!(is_literal_quotable("keeps \n and \t"));
}

#[test]
fn writer_emit_parse_normalize_is_identity() {
    let fixtures = [
        DEFAULT_CONTEXT_ALERT_TEMPLATE,
        "# Heading\n\n**bold**\n\n```rust\nfn main() {}\n```\n\n- a\n\n- b",
        "\nstarts with a newline",
        "ends with a newline\n",
        "interior\n\n\nblank lines",
        "trailing spaces   \nand\ttabs\t",
        "emoji 🚨 and acentos: contexto crítico",
        "ends in a quote '",
        "ends in two quotes ''",
        "%MEMBER% %UNKNOWN% 100%",
    ];
    for fixture in fixtures {
        assert!(
            is_literal_quotable(fixture),
            "fixture not quotable: {:?}",
            fixture
        );
        for crlf in [false, true] {
            let emitted = quote_literal(fixture, crlf);
            let source = format!("[messages.x]\ntemplate = {}\n", emitted);
            let root: toml::Value = toml::from_str(&source)
                .unwrap_or_else(|e| panic!("emit did not parse for {:?}: {}", fixture, e));
            let parsed = root["messages"]["x"]["template"].as_str().expect("string");
            assert_eq!(
                normalize_parsed_template(parsed),
                fixture,
                "round trip failed for {:?} (crlf={})",
                fixture,
                crlf
            );
        }
    }
}

#[test]
fn duplicate_message_header_is_strict_invalid() {
    let source = "schema_version = 1\n\n[messages.context-alert]\ntemplate = \
        'a'\n\n[messages.context-alert]\ntemplate = 'b'\n";
    let model = analyze_source(source);
    assert!(
        model.strict_invalid.is_some(),
        "a duplicate header must never become a silent last-wins"
    );
}

#[test]
fn append_into_inline_table_file_publishes_nothing() {
    let dir = tempdir();
    let source = "schema_version = 1\nmessages = { \"context-alert\" = { template = \"X\" } }\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let specs = two_specs();
    let report = provision(dir.path(), &specs, None).expect("provision");
    assert!(
        report.writer_disabled.is_some(),
        "an append that would invalidate the file must disable the writer"
    );
    assert_eq!(read(&main_path(dir.path())), source);
    assert!(!report.outcomes.contains_key(EXTRA_MESSAGE_ID));
}

#[test]
fn writer_never_publishes_unparseable_text() {
    // The refresh path: a default that cannot be written as a TOML literal
    // is refused before anything reaches disk.
    const UNQUOTABLE: &str = "a ''' inside the value";
    let unquotable_specs = [MessageSpec {
        id: CONTEXT_ALERT_MESSAGE_ID,
        default_template: UNQUOTABLE,
        known_default_sha256: &[PINNED_SHA256],
        tokens: &[TOKEN_MEMBER],
        doc_comment: CONTEXT_ALERT_DOC_COMMENT,
    }];
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let seeded = read(&main_path(dir.path()));
    let report = provision(dir.path(), &unquotable_specs, None).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), seeded);

    // The append path: a source whose `messages` is an inline table cannot
    // be extended, and the candidate is discarded on re-parse.
    let dir = tempdir();
    let source = "schema_version = 1\nmessages = { other = { template = \"X\" } }\n";
    std::fs::write(main_path(dir.path()), source).expect("write");
    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), source);

    // And the gate itself, exercised directly.
    let previous = BTreeSet::from([CONTEXT_ALERT_MESSAGE_ID.to_string()]);
    let intended = BTreeMap::from([(CONTEXT_ALERT_MESSAGE_ID.to_string(), "value".to_string())]);
    assert!(verify_candidate("[messages.context-alert\n", &previous, &intended).is_err());
    assert!(verify_candidate("schema_version = 1\n", &previous, &intended).is_err());
    assert!(verify_candidate(
        "[messages.context-alert]\ntemplate = 'other'\n",
        &previous,
        &intended
    )
    .is_err());
    assert!(verify_candidate(
        "[messages.context-alert]\ntemplate = 'value'\n",
        &previous,
        &intended
    )
    .is_ok());
}

#[test]
fn bom_file_round_trips() {
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    let seeded = read(&main_path(dir.path()));
    let with_bom = format!("\u{feff}{}", seeded);
    std::fs::write(main_path(dir.path()), &with_bom).expect("write BOM");

    let model = analyze_source(&with_bom);
    assert_eq!(model.strict_invalid, None);
    assert_eq!(
        model.templates.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&DEFAULT_CONTEXT_ALERT_TEMPLATE.to_string()),
        "the BOM must be invisible to hashing and reconciliation"
    );

    // A refresh still targets the right span, and the BOM survives.
    let specs = next_specs();
    let report = provision(dir.path(), &specs, None).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with('\u{feff}'));
    assert!(after.contains(NEXT_CONTEXT_ALERT_TEMPLATE));

    let report = provision(dir.path(), &specs, None).expect("second run");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );
    assert_eq!(read(&main_path(dir.path())), after);
}

#[test]
fn scalar_entry_does_not_disable_the_file() {
    let dir = tempdir();
    // The single most natural operator mistake: assigning the text to the
    // id instead of to `template`.
    let source = "schema_version = 1\n\n[messages.healthy]\ntemplate = \
        '''\nHEALTHY\n'''\n\n[messages]\ncontext-alert = \"my own text\"\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let model = analyze_source(source);
    assert_eq!(model.strict_invalid, None);
    assert_eq!(
        model.entries.get("healthy"),
        Some(&EntryValue::Template("HEALTHY".to_string()))
    );
    assert_eq!(
        model.entries.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryValue::Malformed)
    );

    let report = ensure_injected_messages(dir.path()).expect("provision");
    // The damaged entry is preserved verbatim and the four ids this binary
    // knows and the file lacks are appended after it.
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with(source));
    assert_eq!(
        appended_headers(&after[source.len()..]),
        expected_appended_headers()
    );
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit)
    );
    // The damaged entry renders the embedded default.
    assert!(!load_registry_from_dir(dir.path()).contains_key(CONTEXT_ALERT_MESSAGE_ID));
}

#[test]
fn rendered_output_passes_validate_pty_input_text() {
    let hostile =
        "a\u{1b}[31m\u{202e}\u{2066}\u{2028}\u{7f}\u{85}\rb %MEMBER% %%AC_RESPONSE::x::START%%";
    let templates = [
        DEFAULT_CONTEXT_ALERT_TEMPLATE,
        NEXT_CONTEXT_ALERT_TEMPLATE,
        "## %MEMBER%\n\n- **%WORKGROUP%** at `%OBSERVED%`",
        hostile,
        &"🚨".repeat(4096),
        "%FOO% %member% 100%",
    ];
    for template in templates {
        let rendered = render_with_template(template, &alert_values());
        assert!(
            validate_pty_input_text(&rendered).is_ok(),
            "rendered output rejected by AC's own validator for {:?}: {:?}",
            &template[..template.len().min(40)],
            validate_pty_input_text(&rendered)
        );
    }
    // And through the id-resolving entry point, which is what mailbox calls.
    assert!(validate_pty_input_text(&render(CONTEXT_ALERT_MESSAGE_ID, &alert_values())).is_ok());

    // The 6.1 gate-2 class: templates `trim()` calls non-blank because the
    // bidi and format scalars are not `White_Space`, but which `sanitize`
    // empties. Exercised through `render_resolved`, because
    // `render_with_template` deliberately has no blank gate, so requiring
    // it there would be requiring the wrong thing.
    for template in [
        "\u{202e}",
        "\u{200e}\u{200f}",
        "\u{7}",
        "\u{61c}\u{2066}\u{2069}",
    ] {
        assert!(
            !template.trim().is_empty(),
            "fixture must survive gate 1: {:?}",
            template
        );
        assert!(
            render_with_template(template, &alert_values()).is_empty(),
            "fixture must sanitize to empty: {:?}",
            template
        );
        let rendered = render_resolved(CONTEXT_ALERT_MESSAGE_ID, template, true, &alert_values());
        assert!(!rendered.is_empty(), "gate 2 left it empty: {:?}", template);
        assert!(
            validate_pty_input_text(&rendered).is_ok(),
            "gate-2 fallback rejected by AC's own validator for {:?}",
            template
        );
    }
}

#[test]
fn setext_heading_is_not_a_conflict_marker() {
    let dir = tempdir();
    let source = "schema_version = 1\n\n[messages.context-alert]\ntemplate = \
        '''\nAlerta\n=======\ncuerpo del mensaje\n'''\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    assert!(!has_conflict_markers(source));
    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        load_registry_from_dir(dir.path()).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"Alerta\n=======\ncuerpo del mensaje".to_string())
    );
    // The paired rule still catches a real conflict.
    assert!(has_conflict_markers(
        "<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> other\n"
    ));
}

#[test]
fn second_provisioning_run_writes_nothing() {
    let _serialized = COUNTING_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");

    start_counting();
    ensure_injected_messages(dir.path()).expect("second run");
    let writes = stop_counting(dir.path());
    assert!(
        writes.is_empty(),
        "an unchanged second run must write nothing, wrote: {:?}",
        writes
    );

    // A first run does write all three, which is what makes the assertion
    // above meaningful.
    let dir = tempdir();
    start_counting();
    ensure_injected_messages(dir.path()).expect("first run");
    let writes = stop_counting(dir.path());
    assert_eq!(
        writes.len(),
        3,
        "expected file, sidecar and companion: {:?}",
        writes
    );
}

#[test]
fn sidecar_write_failure_does_not_freeze_entry() {
    let dir = tempdir();
    ensure_injected_messages(dir.path()).expect("seed");
    // Force every later sidecar write to fail by occupying its path with a
    // directory. The main file still publishes.
    std::fs::remove_file(state_path(dir.path())).expect("drop sidecar");
    std::fs::create_dir(state_path(dir.path())).expect("block sidecar");

    let specs = next_specs();
    let report = provision(dir.path(), &specs, None).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );
    assert!(read(&main_path(dir.path())).contains(NEXT_CONTEXT_ALERT_TEMPLATE));

    // With the sidecar still unwritable, the next run converges through the
    // recognizer rather than freezing the entry as a user edit. The pass
    // must ship a THIRD default that lists the on-disk content among its
    // known ones: reusing `specs` would land in `AlreadyCurrent`, which
    // passes even when no recognizer exists.
    assert_eq!(
        sha256_hex(NEXT_CONTEXT_ALERT_TEMPLATE.as_bytes()),
        NEXT_SHA256,
        "the pinned hash of the second default drifted"
    );
    let third = third_specs();
    assert!(
        third[0].known_default_sha256.contains(&NEXT_SHA256),
        "the recognizer must list the on-disk content"
    );
    let report = provision(dir.path(), &third, None).expect("second run");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed),
        "recognized content one version behind must refresh, not freeze"
    );
    assert!(read(&main_path(dir.path())).contains(THIRD_CONTEXT_ALERT_TEMPLATE));

    let report = provision(dir.path(), &KNOWN_MESSAGES, None).expect("downgrade run");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit),
        "an unrecognized hash is still preserved"
    );
}

/// N17. What makes the single retry in gate 2 total: the fallback can never
/// itself render empty, so no second failure is reachable and the gate
/// needs no loop.
#[test]
fn every_shipped_default_renders_non_empty() {
    for spec in KNOWN_MESSAGES.iter() {
        let rendered = render_with_template(spec.default_template, &alert_values());
        assert!(!rendered.is_empty(), "`{}` rendered empty", spec.id);
        assert!(
            validate_pty_input_text(&rendered).is_ok(),
            "`{}` rejected by AC's own validator: {:?}",
            spec.id,
            validate_pty_input_text(&rendered)
        );
    }
}

/// N18. The gate itself, on the pure seam, with no global registry involved.
#[test]
fn template_that_sanitizes_to_empty_falls_back_to_default() {
    let values = alert_values();
    let expected = render_with_template(DEFAULT_CONTEXT_ALERT_TEMPLATE, &values);

    let rendered = render_resolved(CONTEXT_ALERT_MESSAGE_ID, "\u{202e}", true, &values);
    assert_eq!(rendered, expected);
    assert!(!rendered.is_empty());

    // It warned: the key is registered afterwards, whatever the test order.
    assert!(
        !warn_once(
            format!("empty:{}", CONTEXT_ALERT_MESSAGE_ID),
            "probe, never emitted"
        ),
        "gate 2 must register its warn-once key"
    );

    // With `from_registry = false` the same input returns "". The gate is
    // deliberately not applied to an embedded default: a shipped default
    // that sanitizes to nothing is a bug to be caught by N17, not a
    // condition to paper over at runtime.
    assert_eq!(
        render_resolved(CONTEXT_ALERT_MESSAGE_ID, "\u{202e}", false, &values),
        ""
    );
}

/// N19. `warn_once`'s return value is the only observable the mechanism has,
/// and the log is this feature's only channel to the operator.
///
/// Every key and token below is unique to this test, because the set is
/// process-global and shared across the whole test binary.
#[test]
fn warn_once_fires_once_per_key() {
    assert!(warn_once("n19-unique-key-alpha".to_string(), "first"));
    assert!(!warn_once("n19-unique-key-alpha".to_string(), "second"));
    assert!(warn_once(
        "n19-unique-key-beta".to_string(),
        "different key"
    ));

    // The key is the token, including its `%` delimiters, so the set cannot
    // grow with the number of templates.
    let rendered = render_with_template("%N19_ALPHA% and %N19_BETA% and %N19_ALPHA%", &[]);
    assert_eq!(rendered, "%N19_ALPHA% and %N19_BETA% and %N19_ALPHA%");
    for token in ["%N19_ALPHA%", "%N19_BETA%"] {
        assert!(
            !warn_once(format!("token:{}", token), "probe, never emitted"),
            "{} did not register exactly once",
            token
        );
    }
}

/// N20. The Windows reparse-point branch of `is_link_or_reparse`, exercised
/// with a junction, which needs no privileges.
///
/// Recorded residue: this pins the class "a real reparse point occupies the
/// path" and that `is_link_or_reparse` rejects it. It does not pin WHICH of
/// that function's two branches fires, since whether `std` classifies a
/// mount point as a symlink is not asserted here either way.
#[cfg(windows)]
#[test]
fn reparse_point_in_the_path_is_rejected() {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let dir = tempdir();
    let target = dir.path().join("junction-target");
    std::fs::create_dir(&target).expect("junction target");
    let link = main_path(dir.path());

    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .expect("run mklink");
    // Failing rather than skipping, so the coverage cannot silently
    // evaporate on a machine where the junction cannot be created.
    assert!(
        status.status.success(),
        "mklink /J failed: {}{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );

    let metadata = std::fs::symlink_metadata(&link).expect("symlink_metadata");
    assert!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0,
        "the path does not carry FILE_ATTRIBUTE_REPARSE_POINT"
    );
    assert!(is_link_or_reparse(&metadata));

    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert!(load_registry_from_dir(dir.path()).is_empty());

    // Neither followed nor replaced.
    let after = std::fs::symlink_metadata(&link).expect("still there");
    assert!(is_link_or_reparse(&after));
    assert!(target.is_dir());
    assert!(
        std::fs::read_dir(&target)
            .expect("read target")
            .next()
            .is_none(),
        "nothing was written through the junction"
    );
}

#[test]
fn absent_version_keys_are_treated_as_current_and_never_written_back() {
    let dir = tempdir();
    let source = format!(
        "[messages.context-alert]\ntemplate = '''\n{}\n'''\n",
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );
    // Absent version keys are never written back; the four missing ids are
    // appended after the operator's own bytes, which stay a prefix.
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with(&source));
    assert!(!after.contains("schema_version = "));
    assert!(!after.contains("coverage_version = "));
    assert_eq!(
        appended_headers(&after[source.len()..]),
        expected_appended_headers()
    );

    // A non-integer version key is strict-invalid, unlike an absent one.
    assert!(analyze_source("schema_version = \"1\"\n")
        .strict_invalid
        .is_some());
    assert!(analyze_source("coverage_version = true\n")
        .strict_invalid
        .is_some());
}

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

#[test]
fn known_messages_has_five_entries_with_unique_ids() {
    assert_eq!(KNOWN_MESSAGES.len(), 5);
    let ids: Vec<&str> = KNOWN_MESSAGES.iter().map(|spec| spec.id).collect();
    let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "duplicate ids: {:?}", ids);
    assert_eq!(
        ids,
        vec![
            CONTEXT_ALERT_MESSAGE_ID,
            CI_STARTED_MESSAGE_ID,
            CI_FINISHED_MESSAGE_ID,
            BRANCH_STALE_MESSAGE_ID,
            NOTICE_BLIND_GAP_MESSAGE_ID,
        ]
    );
    // Every documented token is a token the spec declares, which is what
    // keeps the generated `#   %TOKEN%` lines honest.
    for spec in &KNOWN_MESSAGES {
        assert!(!spec.default_template.is_empty(), "{}", spec.id);
        assert!(!spec.tokens.is_empty(), "{}", spec.id);
        for token in spec.tokens {
            assert!(
                spec.doc_comment.contains(token),
                "{} documents every token; {} is missing",
                spec.id,
                token
            );
        }
    }
}

/// #2141 replaced the older "exactly one digest" rule. That rule encoded "no
/// shipped default has moved yet", which stopped being true when the
/// branch-stale wording changed; the rule worth keeping is that the list ENDS
/// at what ships today, so reconciliation still recognizes every default ever
/// shipped and refreshes a pristine entry to the current one.
#[test]
fn each_default_digest_list_ends_at_the_current_template() {
    for spec in &KNOWN_MESSAGES {
        assert!(
            !spec.known_default_sha256.is_empty(),
            "`{}` must list at least the digest it ships today",
            spec.id
        );
        assert_eq!(
            spec.known_default_sha256.last().copied(),
            Some(sha256_hex(spec.default_template.as_bytes()).as_str()),
            "`{}`: the LAST digest must be the shipped default's, newest last",
            spec.id
        );
        let mut seen = std::collections::HashSet::new();
        for digest in spec.known_default_sha256 {
            assert!(
                seen.insert(*digest),
                "`{}` lists {} twice; every entry is a distinct shipped default",
                spec.id,
                digest
            );
        }
    }
}

#[test]
fn canonical_seed_bytes_and_reference_bytes_regenerated() {
    // Both pin transcribed bytes, read by a human off the regenerated files
    // and checked against the specification: a comparison against
    // `canonical_*_bytes` alone cannot fail, so a header or ordering edit
    // would drift from the spec with nothing going red.
    assert_eq!(EXPECTED_SEED.len(), 3712, "the pinned seed is 3712 bytes");
    assert_eq!(canonical_seed_bytes(&KNOWN_MESSAGES), EXPECTED_SEED);
    assert_eq!(
        EXPECTED_REFERENCE.len(),
        3444,
        "the pinned companion is 3444 bytes"
    );
    assert_eq!(
        canonical_reference_bytes(&KNOWN_MESSAGES),
        EXPECTED_REFERENCE
    );
}

#[test]
fn coverage_version_is_two() {
    assert_eq!(COVERAGE_VERSION, 2);
    assert_eq!(KNOWN_MESSAGES.len(), 5);
}

#[test]
fn newer_coverage_version_literal_raised_to_three() {
    assert_eq!(NEWER_COVERAGE_VERSION, 3);
    // A const block, so the invariant is checked when it can still be false:
    // a runtime assertion over two constants can never fail and would quietly
    // stop meaning anything.
    const {
        assert!(
            NEWER_COVERAGE_VERSION > COVERAGE_VERSION,
            "the fixture must stay strictly above COVERAGE_VERSION: `writer_blocked` is gated on \
             a strict `>`, so equality leaves the writer enabled and the test stops exercising \
             the newer-than-this-binary branch"
        );
    }
}

#[test]
fn existing_operator_file_at_coverage_one_is_appended_not_rewritten() {
    let dir = tempdir();
    let source =
        "schema_version = 1\ncoverage_version = 1\n\n[messages.context-alert]\ntemplate = \
        '''\nOPERATOR %MEMBER% EDITED\n'''\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit)
    );
    for spec in KNOWN_MESSAGES.iter().skip(1) {
        assert_eq!(
            report.outcomes.get(spec.id),
            Some(&EntryOutcome::Appended),
            "{}",
            spec.id
        );
    }

    let after = read(&main_path(dir.path()));
    assert!(
        after.starts_with(source),
        "an operator edit at coverage_version 1 is appended to, never rewritten"
    );
    assert!(!after.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert_eq!(
        appended_headers(&after[source.len()..]),
        expected_appended_headers()
    );
    assert_eq!(
        load_registry_from_dir(dir.path()).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"OPERATOR %MEMBER% EDITED".to_string())
    );
    for spec in KNOWN_MESSAGES.iter().skip(1) {
        assert_eq!(
            load_registry_from_dir(dir.path()).get(spec.id),
            Some(&spec.default_template.to_string()),
            "{}",
            spec.id
        );
    }
}

#[test]
fn unusable_path_preserves_state_and_keeps_rendering() {
    // A directory where the file should be: preserved, writer disabled,
    // rendering unaffected, and never fatal.
    let dir = tempdir();
    std::fs::create_dir(main_path(dir.path())).expect("directory in the way");
    let report = ensure_injected_messages(dir.path()).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert!(main_path(dir.path()).is_dir());
    assert!(load_registry_from_dir(dir.path()).is_empty());
    assert_eq!(
        render(CONTEXT_ALERT_MESSAGE_ID, &alert_values()),
        render_with_template(DEFAULT_CONTEXT_ALERT_TEMPLATE, &alert_values())
    );
}
