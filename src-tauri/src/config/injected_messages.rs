//! #1157 - operator-editable templates for the messages AgentsCommander injects
//! into a coordinator PTY.
//!
//! The registry lives next to the executable, in `config_dir()`, as
//! `injected-messages.toml`. It is seeded on first start, refreshed per entry
//! when a newer default ships, and never overwritten once the operator has
//! edited an entry. Rendering never fails: any resolution problem degrades to
//! the embedded default.
//!
//! Three properties carry the design and must survive any future edit:
//!
//! - **Surgical writes only.** The writer locates a `template` value through
//!   [`toml::Spanned`] and replaces exactly that literal. A whole-document
//!   `toml::to_string` would drop every comment, reorder keys and rewrite `'''`
//!   as `"""`, which is precisely what the seeded file exists to preserve.
//! - **Verify before publish.** No candidate is ever written without being
//!   re-parsed first, so AC can never turn the operator's own config into an
//!   invalid file.
//! - **One authoritative forbidden-scalar set.** [`sanitize`] derives its filter
//!   from `crate::pty::inject::is_forbidden_pty_scalar`, the same predicate
//!   `validate_pty_input_text` uses, so the two can never drift.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::local_config_io::write_file_atomic;

pub(crate) const INJECTED_MESSAGES_FILENAME: &str = "injected-messages.toml";
pub(crate) const INJECTED_MESSAGES_REFERENCE_FILENAME: &str = "injected-messages.default.toml";
pub(crate) const INJECTED_MESSAGES_STATE_FILENAME: &str = ".agentscommander-injected-messages.json";

/// Versions the file's STRUCTURE only. Independent of AC's semver and of the
/// sidecar's own version. Never bumped by adding message ids or by improving a
/// default; content is versioned per entry by sha256.
pub(crate) const SCHEMA_VERSION: u32 = 1;
/// Versions the SET of covered ids.
pub(crate) const COVERAGE_VERSION: u32 = 2;

pub(crate) const CONTEXT_ALERT_MESSAGE_ID: &str = "context-alert";
pub(crate) const CI_STARTED_MESSAGE_ID: &str = "ci-started";
pub(crate) const CI_FINISHED_MESSAGE_ID: &str = "ci-finished";
pub(crate) const BRANCH_STALE_MESSAGE_ID: &str = "branch-stale";
pub(crate) const NOTICE_BLIND_GAP_MESSAGE_ID: &str = "notice-blind-gap";

pub(crate) const TOKEN_MEMBER: &str = "%MEMBER%";
pub(crate) const TOKEN_WORKGROUP: &str = "%WORKGROUP%";
pub(crate) const TOKEN_THRESHOLDS: &str = "%THRESHOLDS%";
pub(crate) const TOKEN_OBSERVED: &str = "%OBSERVED%";
pub(crate) const TOKEN_REPO: &str = "%REPO%";
pub(crate) const TOKEN_BRANCH: &str = "%BRANCH%";
pub(crate) const TOKEN_SHA: &str = "%SHA%";
pub(crate) const TOKEN_AT: &str = "%AT%";
pub(crate) const TOKEN_BASE: &str = "%BASE%";
pub(crate) const TOKEN_BEHIND: &str = "%BEHIND%";
pub(crate) const TOKEN_GAP: &str = "%GAP%";
pub(crate) const TOKEN_SINCE: &str = "%SINCE%";

/// The only embedded copy of the context-alert wording. 125 characters, 125
/// UTF-8 bytes, sha256 `e672581d47e7e4a4749b510f23eff72982ff3fa5261109122b3bdf8fdfda153f`.
pub(crate) const DEFAULT_CONTEXT_ALERT_TEMPLATE: &str = "[AC context alert] `%MEMBER%` in \
    `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No action taken; you decide any follow-up.";

/// sha256 `e8d7993332268690db1220277b0c3926c99ce4d1e3e4af8c0978c22e3b01c50a`.
pub(crate) const DEFAULT_CI_STARTED_TEMPLATE: &str = "[AgentsCommander] CI started on %REPO% \
    %BRANCH% (commit %SHA%) at %AT%. Status notice only; no action is required.";

/// sha256 `8e9df5e4986e93999f10401d31b12f3f629f43cde6519691802ddf68e77f0b85`. The
/// second sentence is mandatory and is not filler: `conclusion` is never read,
/// so "CI finished" must never be read as "CI passed".
pub(crate) const DEFAULT_CI_FINISHED_TEMPLATE: &str = "[AgentsCommander] CI finished on %REPO% \
    %BRANCH% (commit %SHA%) at %AT%. AgentsCommander does not track pass or fail: run 'gh run list \
    --branch %BRANCH%' before acting on this.";

/// sha256 `3c03dc790a9d3b36edbf1174dc3e661572ad0f93e70e1b200daff8945ff6e468`. The second sentence
/// is conditional on purpose: #2141
/// suppresses the notice for a branch with no commits of its own, but a branch
/// WITH its own commits and no runs still receives it, so the old wording
/// asserted a premise that need not hold.
pub(crate) const DEFAULT_BRANCH_STALE_TEMPLATE: &str = "[AgentsCommander] %REPO% %BRANCH% is now \
    %BEHIND% commits behind %BASE% as of %AT%. If CI runs on this branch, it is validating an \
    out-of-date base.";

/// sha256 `e895fefbd3e482568d4e21ac22f3826d10d8a7911b3303239726a2b8ade49aff`. It
/// begins with ONE space because it is concatenated onto a sentence that
/// already ends in a period. A suffix id, not twin ids: with twins an operator
/// who edits one and forgets the other gets two silently diverging texts.
pub(crate) const DEFAULT_NOTICE_BLIND_GAP_TEMPLATE: &str = " AgentsCommander could not reach \
    GitHub for %GAP%; the previous confirmed state is from %SINCE%.";

/// The only bound between an operator's template and the coordinator's PTY.
/// `PTY_INPUT_MAX_BYTES` is enforced inside `validate_pty_input_text`, which the
/// internal-notice path does not call, so this cap is load-bearing. It applies
/// to the rendered string BEFORE `format_wake_content` adds its framing bytes.
pub(crate) const MAX_RENDERED_BYTES: usize = 4096;

/// A message AC knows how to render, with every default it has ever shipped.
struct MessageSpec {
    id: &'static str,
    default_template: &'static str,
    /// sha256 of every default ever shipped for this id, newest last. Case 3 of
    /// reconciliation fires on any of them, which is what makes reconciliation
    /// converge without a cross-process lock and what keeps a missing sidecar
    /// from freezing an untouched entry forever.
    known_default_sha256: &'static [&'static str],
    tokens: &'static [&'static str],
    doc_comment: &'static str,
}

const CONTEXT_ALERT_DOC_COMMENT: &str = "\
# Injected into the orchestrator's terminal when a member crosses a configured
# context-usage threshold.
# Placeholders:
#   %MEMBER%      name of the observed member, e.g. dev-rust
#   %WORKGROUP%   room name, e.g. room-2-dev-team
#   %THRESHOLDS%  thresholds just crossed, already formatted, e.g. 50%, 75%
#   %OBSERVED%    observed context use, e.g. 91% (best-effort human signal)";

const CI_STARTED_DOC_COMMENT: &str = "\
# Injected into the orchestrator's terminal when CI starts running on a room repo's
# exact HEAD.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %SHA%     abbreviated commit SHA, 7 characters
#   %AT%      observation time, local with a numeric UTC offset";

const CI_FINISHED_DOC_COMMENT: &str = "\
# Injected into the orchestrator's terminal when CI stops running on a room repo's
# exact HEAD. It never says whether CI passed.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %SHA%     abbreviated commit SHA, 7 characters
#   %AT%      observation time, local with a numeric UTC offset";

const BRANCH_STALE_DOC_COMMENT: &str = "\
# Injected into the orchestrator's terminal when a room repo's branch falls behind
# the repository's default branch.
# Placeholders:
#   %REPO%    repository nwo, e.g. mblua/AgentsCommander
#   %BRANCH%  branch name, e.g. feature/2083-2064-remote-alerts
#   %BASE%    comparison base, already remote-qualified, e.g. main on GitHub
#   %BEHIND%  commit count behind, e.g. 4
#   %AT%      observation time, local with a numeric UTC offset";

const NOTICE_BLIND_GAP_DOC_COMMENT: &str = "\
# Appended to a CI or branch-stale notice when AgentsCommander could not reach
# GitHub for longer than ten minutes before the notice.
# Placeholders:
#   %GAP%    how long GitHub was unreachable, e.g. 47 minutes
#   %SINCE%  time of the previous confirmed state, local with a numeric UTC offset";

const KNOWN_MESSAGES: [MessageSpec; 5] = [
    MessageSpec {
        id: CONTEXT_ALERT_MESSAGE_ID,
        default_template: DEFAULT_CONTEXT_ALERT_TEMPLATE,
        known_default_sha256: &["e672581d47e7e4a4749b510f23eff72982ff3fa5261109122b3bdf8fdfda153f"],
        tokens: &[
            TOKEN_MEMBER,
            TOKEN_WORKGROUP,
            TOKEN_THRESHOLDS,
            TOKEN_OBSERVED,
        ],
        doc_comment: CONTEXT_ALERT_DOC_COMMENT,
    },
    MessageSpec {
        id: CI_STARTED_MESSAGE_ID,
        default_template: DEFAULT_CI_STARTED_TEMPLATE,
        known_default_sha256: &["e8d7993332268690db1220277b0c3926c99ce4d1e3e4af8c0978c22e3b01c50a"],
        tokens: &[TOKEN_REPO, TOKEN_BRANCH, TOKEN_SHA, TOKEN_AT],
        doc_comment: CI_STARTED_DOC_COMMENT,
    },
    MessageSpec {
        id: CI_FINISHED_MESSAGE_ID,
        default_template: DEFAULT_CI_FINISHED_TEMPLATE,
        known_default_sha256: &["8e9df5e4986e93999f10401d31b12f3f629f43cde6519691802ddf68e77f0b85"],
        tokens: &[TOKEN_REPO, TOKEN_BRANCH, TOKEN_SHA, TOKEN_AT],
        doc_comment: CI_FINISHED_DOC_COMMENT,
    },
    MessageSpec {
        id: BRANCH_STALE_MESSAGE_ID,
        default_template: DEFAULT_BRANCH_STALE_TEMPLATE,
        // Two digests, newest last: #2141 is the first change to move a shipped
        // default's bytes. The v1 hash stays so a pristine entry whose sidecar is
        // gone is still recognized and refreshed, instead of freezing forever as
        // a false "user edit".
        known_default_sha256: &[
            "a2f428dcf62be65a968b2ee2cad37a1e11c85a74e3d5643567695325348a258f",
            "3c03dc790a9d3b36edbf1174dc3e661572ad0f93e70e1b200daff8945ff6e468",
        ],
        tokens: &[TOKEN_REPO, TOKEN_BRANCH, TOKEN_BASE, TOKEN_BEHIND, TOKEN_AT],
        doc_comment: BRANCH_STALE_DOC_COMMENT,
    },
    MessageSpec {
        id: NOTICE_BLIND_GAP_MESSAGE_ID,
        default_template: DEFAULT_NOTICE_BLIND_GAP_TEMPLATE,
        known_default_sha256: &["e895fefbd3e482568d4e21ac22f3826d10d8a7911b3303239726a2b8ade49aff"],
        tokens: &[TOKEN_GAP, TOKEN_SINCE],
        doc_comment: NOTICE_BLIND_GAP_DOC_COMMENT,
    },
];

const FILE_HEADER_COMMENT: &str = "\
# AgentsCommander injected PTY message templates.
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
# \"100%\" is safe. AgentsCommander's path placeholders (%AC_REPLICA_ROOT% and
# friends) do NOT apply here.
#
# See injected-messages.default.toml for the current canonical set.";

const REFERENCE_HEADER_COMMENT: &str = "\
# AgentsCommander injected PTY message templates - CANONICAL REFERENCE.
#
# GENERATED FILE, DO NOT EDIT. AgentsCommander rewrites it whenever the shipped
# defaults change, and never reads it back. It exists so you can see the current
# canonical set next to your own injected-messages.toml.
#
# To change what AgentsCommander injects, edit injected-messages.toml instead.";

/// What reconciliation did to one entry. File-level facts, above all a disabled
/// writer, live on [`ProvisionReport`], not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryOutcome {
    /// The whole file was absent and has just been seeded.
    Seeded,
    /// The id was missing from an existing file and was appended.
    Appended,
    /// On-disk content already equals the current default.
    AlreadyCurrent,
    /// On-disk content was a recognized shipped default and was rewritten.
    Refreshed,
    /// On-disk content is the operator's, or unusable, and was left untouched.
    PreservedUserEdit,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProvisionReport {
    /// `Some(reason)` when nothing may be written to the main file this run.
    pub writer_disabled: Option<String>,
    pub outcomes: BTreeMap<String, EntryOutcome>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InjectedMessagesState {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    messages: BTreeMap<String, MessageStateEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct MessageStateEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seeded_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_observed_sha256: Option<String>,
}

/// Which ids `reseed` resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReseedTarget {
    Id(String),
    All,
}

// ---------------------------------------------------------------------------
// Pure rendering engine
// ---------------------------------------------------------------------------

/// True for `%`, one or more of `A-Za-z0-9_`, `%`. The character class is wider
/// than the substitution grammar on purpose: substitution matches known tokens
/// case-sensitively, while recognition for the unknown-token warning is
/// case-insensitive, so an operator who types `%member%` is told about it
/// instead of finding a literal `%member%` in a coordinator's terminal.
fn unknown_token_len(rest: &str) -> Option<usize> {
    let mut chars = rest.char_indices();
    if !matches!(chars.next(), Some((_, '%'))) {
        return None;
    }
    let mut body = 0usize;
    for (offset, ch) in chars {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            body += 1;
            continue;
        }
        if ch == '%' && body > 0 {
            return Some(offset + 1);
        }
        return None;
    }
    None
}

/// Single left-to-right pass. Substituted values are never rescanned, so a value
/// that itself contains `%WORKGROUP%` survives verbatim. The chained
/// `for (k, v) in tokens { s = s.replace(k, v) }` form is prohibited: it
/// rescans, and corrupts a template such as `%THRESHOLDS%OBSERVED%` even with
/// today's upstream-validated values.
fn expand_tokens(template: &str, values: &[(&str, &str)]) -> (String, Vec<String>) {
    let mut out = String::with_capacity(template.len());
    let mut unknown: Vec<String> = Vec::new();
    let mut index = 0usize;

    while index < template.len() {
        let rest = &template[index..];
        if rest.starts_with('%') {
            if let Some((token, value)) = values.iter().find(|(token, _)| rest.starts_with(token)) {
                out.push_str(value);
                index += token.len();
                continue;
            }
            if let Some(length) = unknown_token_len(rest) {
                let token = &rest[..length];
                if !unknown.iter().any(|seen| seen == token) {
                    unknown.push(token.to_string());
                }
                out.push_str(token);
                index += length;
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else { break };
        out.push(ch);
        index += ch.len_utf8();
    }

    (out, unknown)
}

const RESPONSE_MARKER_BODY: &str = "AC_RESPONSE::";

/// Collapses, for every occurrence of `AC_RESPONSE::`, the maximal run of `%`
/// immediately preceding it down to a single `%` when that run is two
/// characters or longer. Runs of length 0 or 1 are left untouched and nothing
/// else in the text changes.
///
/// A single `replace("%%AC_RESPONSE::", "%AC_RESPONSE::")` is not sufficient:
/// `%%%AC_RESPONSE::` becomes `%%AC_RESPONSE::` and matches the scanner again,
/// and the bar is low because expansion runs first, so `%OBSERVED%` expands to
/// `91%` and gets a third `%` from an operator who typed two. The obvious
/// `while contains { replace }` repair is prohibited as a denial of service:
/// truncation is step 4, so at step 3 the text is still the operator's whole
/// template with no bound on its length.
///
/// This form is O(n) and a fixed point by construction: the runs it produces
/// are already of length 1 or 0, so sanitizing twice equals sanitizing once.
fn collapse_response_marker_prefix(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(hit) = input[cursor..].find(RESPONSE_MARKER_BODY) {
        let at = cursor + hit;
        let mut run_start = at;
        // `%` is ASCII 0x25, a byte no multi-byte UTF-8 scalar can contain, so
        // walking back over bytes can never land inside a scalar. Bounding the
        // walk at `cursor` also keeps it inside the not-yet-copied region; it
        // changes no result, since the byte before `cursor` is always the `:`
        // ending the previous marker.
        while run_start > cursor && bytes[run_start - 1] == b'%' {
            run_start -= 1;
        }
        out.push_str(&input[cursor..run_start]);
        if run_start < at {
            out.push('%');
        }
        out.push_str(RESPONSE_MARKER_BODY);
        cursor = at + RESPONSE_MARKER_BODY.len();
    }
    out.push_str(&input[cursor..]);
    out
}

/// Applied to the fully expanded string, in the order below.
///
/// Step 2 is the correctness core: the internal-notice path reaches
/// `inject_text_into_session` with no validation at all, so without it this
/// module would deliver into a coordinator's terminal exactly the bidi and
/// separator class AC refuses on every other injection route.
fn sanitize(value: &str) -> String {
    // 1. CRLF to LF.
    let converted = value.replace("\r\n", "\n");
    // 2. Drop every scalar AC's own PTY-input validator forbids, keeping the
    //    two it allows. Derived, never restated.
    let mut out: String = converted
        .chars()
        .filter(|ch| matches!(ch, '\n' | '\t') || !crate::pty::inject::is_forbidden_pty_scalar(*ch))
        .collect();
    // 3. Neutralize the reply-marker prefix `pty/output.rs` scans for in PTY
    //    output, which injected text echoes into. Stays after step 2, which can
    //    bring two `%` together that a stripped scalar separated, as in
    //    `%\u{202e}%AC_RESPONSE::`. Test 6c guards that order.
    if out.contains(RESPONSE_MARKER_BODY) {
        out = collapse_response_marker_prefix(&out);
    }
    // 4. Truncate on a character boundary. `str::floor_char_boundary` is
    //    unstable on the pinned toolchain.
    let mut end = MAX_RENDERED_BYTES.min(out.len());
    while end > 0 && !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out
}

/// Pure seam: expand then sanitize. Tests and future ids feed it arbitrary
/// values, so it may not assume the upstream validation `InternalSystemNotice`
/// applies to `member` and `workgroup`.
pub(crate) fn render_with_template(template: &str, values: &[(&str, &str)]) -> String {
    let (expanded, unknown) = expand_tokens(template, values);
    for token in unknown {
        warn_once(
            format!("token:{}", token),
            &format!(
                "[injected-messages] unknown placeholder {} left as literal text",
                token
            ),
        );
    }
    sanitize(&expanded)
}

/// Blankness gate 2, after rendering. `sanitize` can empty a template that
/// `trim()` called non-blank at the parse boundary, because the bidi and format
/// scalars it strips are not `White_Space`: a template made only of U+202E, of
/// U+200E and U+200F, of U+061C, of U+2066 through U+2069, or of a
/// non-whitespace C0 scalar such as BEL passes gate 1 and renders to nothing.
/// Without this the alert is lost with no warning at all, `format_wake_content`
/// injects a bare `"\n\n\r"`, and `validate_pty_input_text("")` is `Err(Empty)`.
///
/// A pure seam on purpose, so it is testable without installing anything in the
/// process-global registry, which is empty under `cfg(test)` by rule.
fn render_resolved(
    id: &str,
    template: &str,
    from_registry: bool,
    values: &[(&str, &str)],
) -> String {
    let rendered = render_with_template(template, values);
    if !rendered.is_empty() {
        return rendered;
    }
    // One retry, never a loop: a shipped default can never render empty (N17),
    // so no second failure is reachable. `from_registry` is what distinguishes
    // an operator's template from an embedded default rendering empty, and an
    // id absent from `KNOWN_MESSAGES` never reaches here.
    if from_registry {
        if let Some(spec) = spec_for(id) {
            warn_once(
                format!("empty:{}", id),
                &format!(
                    "[injected-messages] the `{}` template renders to nothing after sanitization; \
                     the built-in default is used instead",
                    id
                ),
            );
            return render_with_template(spec.default_template, values);
        }
    }
    rendered
}

/// Resolves `id` from the registry, or from the embedded default, then renders.
/// Never fails and never blocks an alert. Returns a non-empty string for every
/// id in `KNOWN_MESSAGES`; it returns `""` only for an id this binary does not
/// know, which no production call site can produce.
pub(crate) fn render(id: &str, values: &[(&str, &str)]) -> String {
    let (template, from_registry) = match registry().get(id) {
        Some(template) => (template.clone(), true),
        None => match spec_for(id) {
            Some(spec) => (spec.default_template.to_string(), false),
            // Not a message this binary knows; there is nothing to render.
            None => return String::new(),
        },
    };
    render_resolved(id, &template, from_registry, values)
}

fn spec_for(id: &str) -> Option<&'static MessageSpec> {
    KNOWN_MESSAGES.iter().find(|spec| spec.id == id)
}

// ---------------------------------------------------------------------------
// Registry load
// ---------------------------------------------------------------------------

/// Loaded once per process and never reloaded, so editing the file takes effect
/// on the next AC start, consistent with `settings.json`.
///
/// Under `cfg(test)` this is always empty, which makes the exact-bytes
/// assertions in `phone::mailbox` hermetic by construction rather than by luck:
/// under `cargo test` `config_dir()` resolves to a real directory next to the
/// test binary, and any file left there would make them order-dependent.
pub(crate) fn registry() -> &'static BTreeMap<String, String> {
    static REGISTRY: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        #[cfg(test)]
        {
            BTreeMap::new()
        }
        #[cfg(not(test))]
        {
            match crate::config::config_dir() {
                Some(dir) => load_registry_from_dir(&dir),
                None => BTreeMap::new(),
            }
        }
    })
}

/// The registry load with no global state, so resolution is testable.
fn load_registry_from_dir(dir: &Path) -> BTreeMap<String, String> {
    let path = dir.join(INJECTED_MESSAGES_FILENAME);
    let source = match read_source(&path) {
        Ok(Some(source)) => source,
        Ok(None) => return BTreeMap::new(),
        Err(reason) => {
            warn_once(
                format!("file:{}", reason),
                &format!(
                    "[injected-messages] {} is unusable ({}); rendering built-in defaults",
                    display_path(&path),
                    reason
                ),
            );
            return BTreeMap::new();
        }
    };

    let model = analyze_source(&source);
    if let Some(reason) = &model.strict_invalid {
        warn_once(
            format!("invalid:{}:{}", display_path(&path), reason),
            &format!(
                "[injected-messages] {} is invalid ({}); the file is preserved untouched and \
                 built-in defaults are rendered",
                display_path(&path),
                reason
            ),
        );
        return BTreeMap::new();
    }
    model.templates
}

// ---------------------------------------------------------------------------
// Read and render parse
// ---------------------------------------------------------------------------

/// One entry as the read parse sees it. A `toml::Value` parse never fails on
/// entry shape, so no per-entry mistake can disable the whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryValue {
    /// A valid string, already normalized. May be blank.
    Template(String),
    /// Present but unusable: wrong type, no `template` key, or not a table.
    Malformed,
}

#[derive(Debug, Default)]
struct ReadModel {
    /// Preserve the bytes, disable the writer, render embedded defaults.
    strict_invalid: Option<String>,
    /// Preserve the bytes and disable the writer, but keep rendering the
    /// operator's text. Only a `coverage_version` ahead of this binary.
    writer_blocked: Option<String>,
    /// Known ids that render from the file.
    templates: BTreeMap<String, String>,
    /// Every `messages.<id>` present, however malformed.
    entries: BTreeMap<String, EntryValue>,
}

/// Strict-invalid only if BOTH halves of a conflict are present. A single
/// `=======` line is a setext Markdown heading, and Markdown fidelity is this
/// feature's headline promise; git always writes all three markers, so the
/// paired rule misses nothing real.
fn has_conflict_markers(source: &str) -> bool {
    let mut opened = false;
    let mut closed = false;
    for line in source.lines() {
        if line.starts_with("<<<<<<< ") {
            opened = true;
        }
        if line.starts_with(">>>>>>> ") {
            closed = true;
        }
    }
    opened && closed
}

fn integer_key(root: &toml::Value, key: &str) -> Result<Option<i64>, String> {
    match root.get(key) {
        None => Ok(None),
        Some(toml::Value::Integer(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("`{}` must be an integer", key)),
    }
}

fn analyze_source(source: &str) -> ReadModel {
    let mut model = ReadModel::default();

    if has_conflict_markers(source) {
        model.strict_invalid = Some("it contains Git conflict markers".to_string());
        return model;
    }

    let root: toml::Value = match toml::from_str(source) {
        Ok(root) => root,
        Err(e) => {
            model.strict_invalid = Some(format!("TOML parse error: {}", e));
            return model;
        }
    };

    // Absent version keys are treated as this binary's own and are never
    // written back, so an operator who deleted one gets no surprise rewrite.
    // `Option` keeps "absent" and "explicitly 0" distinguishable.
    let schema_version = match integer_key(&root, "schema_version") {
        Ok(value) => value,
        Err(e) => {
            model.strict_invalid = Some(e);
            return model;
        }
    };
    let coverage_version = match integer_key(&root, "coverage_version") {
        Ok(value) => value,
        Err(e) => {
            model.strict_invalid = Some(e);
            return model;
        }
    };
    if schema_version.unwrap_or(SCHEMA_VERSION as i64) > SCHEMA_VERSION as i64 {
        model.strict_invalid = Some(format!(
            "`schema_version` {} is newer than this AgentsCommander understands ({})",
            schema_version.unwrap_or_default(),
            SCHEMA_VERSION
        ));
        return model;
    }
    // A higher `coverage_version` is deliberately NOT strict-invalid: the
    // structure is unchanged, so discarding the operator's text on a downgrade
    // would be data loss at the moment they most want stability.
    if coverage_version.unwrap_or(COVERAGE_VERSION as i64) > COVERAGE_VERSION as i64 {
        model.writer_blocked = Some(format!(
            "`coverage_version` {} is newer than this AgentsCommander understands ({})",
            coverage_version.unwrap_or_default(),
            COVERAGE_VERSION
        ));
    }

    if let Some(toml::Value::Table(messages)) = root.get("messages") {
        for (id, entry) in messages {
            let value = match entry {
                toml::Value::Table(table) => match table.get("template") {
                    Some(toml::Value::String(template)) => {
                        EntryValue::Template(normalize_parsed_template(template))
                    }
                    Some(_) | None => EntryValue::Malformed,
                },
                _ => EntryValue::Malformed,
            };
            if let EntryValue::Template(template) = &value {
                if !template.trim().is_empty() && spec_for(id).is_some() {
                    model.templates.insert(id.clone(), template.clone());
                }
            }
            model.entries.insert(id.clone(), value);
        }
    }

    model
}

// ---------------------------------------------------------------------------
// Normalization, hashing, quoting
// ---------------------------------------------------------------------------

/// Applied exactly once, at the parse boundary, so nothing downstream ever sees
/// a raw parsed template. Missing any consumer produces a permanent false
/// "edited" classification: the default constant is authored without a trailing
/// newline while a `'''` value parses with one.
///
/// The CRLF step exists for the basic-string `\r\n` escape (`template =
/// "a\r\nb"`), the only way a CRLF pair reaches here, since `toml` already
/// collapses CRLF inside multi-line literals. A LONE `\r` is deliberately not
/// stripped: it is a stable hashable byte, `sanitize` removes it at render
/// time, and stripping it here would break the emit/parse/normalize identity.
fn normalize_parsed_template(value: &str) -> String {
    let mut normalized = value.replace("\r\n", "\n");
    if normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized
}

/// Reimplemented rather than reused: `seeded_context_templates`'s copy is a
/// private `fn`.
fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// `'''` quoting is not total. This is a TOML-validity check, explicitly NOT a
/// security filter; security is [`sanitize`]'s job.
fn is_literal_quotable(template: &str) -> bool {
    !template.contains("'''")
        && !template.contains('\r')
        && !template
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
}

/// Emits a freshly quoted literal, never bare text: the span covers the
/// delimiters, so splicing bare text produces a syntactically broken file.
fn quote_literal(template: &str, crlf: bool) -> String {
    if crlf {
        format!("'''\r\n{}\r\n'''", template.replace('\n', "\r\n"))
    } else {
        format!("'''\n{}\n'''", template)
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

/// Returns whether it emitted: `true` on the first use of `key`, `false`
/// afterwards. That return value is the only observable this mechanism has, and
/// the log is the only channel this feature has to the operator, so without it
/// nothing about that channel is testable.
fn warn_once(key: String, message: &str) -> bool {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let warned = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    match warned.lock() {
        Ok(mut guard) => {
            if guard.insert(key) {
                log::warn!("{}", message);
                true
            } else {
                false
            }
        }
        // A poisoned warn-once set must never cost the caller its message.
        Err(_) => {
            log::warn!("{}", message);
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical bytes
// ---------------------------------------------------------------------------

fn message_block(spec: &MessageSpec) -> String {
    format!(
        "\n[messages.{}]\n{}\ntemplate = {}\n",
        spec.id,
        spec.doc_comment,
        quote_literal(spec.default_template, false)
    )
}

/// The exact bytes a clean install is seeded with: UTF-8, LF, one trailing
/// newline. Each block starts with `\n`, which is also what keeps an append
/// from being swallowed by a source that ends inside a comment.
fn canonical_seed_bytes(specs: &[MessageSpec]) -> String {
    let mut out = format!(
        "{}\nschema_version = {}\ncoverage_version = {}\n",
        FILE_HEADER_COMMENT, SCHEMA_VERSION, COVERAGE_VERSION
    );
    for spec in specs {
        out.push_str(&message_block(spec));
    }
    out
}

/// The read-only companion. Never an input, and carries the supported
/// placeholders per id so the file is a complete reference on its own.
fn canonical_reference_bytes(specs: &[MessageSpec]) -> String {
    let mut out = format!(
        "{}\nschema_version = {}\ncoverage_version = {}\n",
        REFERENCE_HEADER_COMMENT, SCHEMA_VERSION, COVERAGE_VERSION
    );
    for spec in specs {
        out.push_str(&format!(
            "\n[messages.{}]\n# Supported placeholders: {}\n{}\ntemplate = {}\n",
            spec.id,
            spec.tokens.join(", "),
            spec.doc_comment,
            quote_literal(spec.default_template, false)
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Write parse
// ---------------------------------------------------------------------------

/// The write parse. `template` is a `Spanned<Value>`, never a `Spanned<String>`:
/// a wrongly typed template must degrade that entry alone. `#[serde(untagged)]`
/// is not an alternative, because it buffers through serde's
/// `ContentDeserializer`, which drops the span hook and yields `span = None`.
#[derive(Deserialize)]
struct WriteDoc {
    #[serde(default)]
    messages: BTreeMap<String, WriteEntry>,
}

#[derive(Deserialize)]
struct WriteEntry {
    #[serde(default)]
    template: Option<toml::Spanned<toml::Value>>,
}

// ---------------------------------------------------------------------------
// Provisioning
// ---------------------------------------------------------------------------

fn read_source(path: &Path) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err("the path is not a regular file".to_string());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("it cannot be inspected: {}", e)),
    }

    let bytes = std::fs::read(path).map_err(|e| format!("it cannot be read: {}", e))?;
    match String::from_utf8(bytes) {
        Ok(source) => Ok(Some(source)),
        Err(e) => Err(format!("it is not valid UTF-8: {}", e)),
    }
}

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

fn load_state(path: &Path) -> InjectedMessagesState {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => InjectedMessagesState::default(),
    }
}

/// Write-if-different. An unconditional write would mean an atomic publish on
/// every launch forever: mtime churn, pointless fsyncs, extra sharing-violation
/// surface, and a warning on every launch in a read-only config directory.
fn write_if_different(path: &Path, contents: &str) -> Result<bool, String> {
    if let Ok(existing) = std::fs::read(path) {
        if existing == contents.as_bytes() {
            return Ok(false);
        }
    }
    publish(path, contents.as_bytes()).map(|()| true)
}

/// The module's single publication point, so a test can count writes without
/// depending on filesystem timestamp resolution.
fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    #[cfg(test)]
    tests::record_write(path);
    write_file_atomic(path, bytes)
}

pub(crate) fn ensure_injected_messages(dir: &Path) -> Result<ProvisionReport, String> {
    provision(dir, &KNOWN_MESSAGES, None)
}

/// Seeds, reconciles and appends, then regenerates the companion.
///
/// `forced` marks ids that must be rewritten to the current default regardless
/// of their content, which is what `reseed` is; a forced run takes a timestamped
/// backup before publishing, because it can overwrite operator text. Refresh
/// deliberately takes none: it only ever fires on content whose hash is provably
/// a shipped default, so there is no operator text to lose.
fn provision(
    dir: &Path,
    specs: &[MessageSpec],
    forced: Option<&BTreeSet<String>>,
) -> Result<ProvisionReport, String> {
    let path = dir.join(INJECTED_MESSAGES_FILENAME);
    let state_path = dir.join(INJECTED_MESSAGES_STATE_FILENAME);
    let mut report = ProvisionReport::default();

    let source = match read_source(&path) {
        Ok(source) => source,
        Err(reason) => {
            report.writer_disabled = Some(reason.clone());
            log::warn!(
                "[injected-messages] {} is unusable ({}); it is left untouched and built-in \
                 defaults are rendered",
                display_path(&path),
                reason
            );
            write_reference(dir, specs);
            return Ok(report);
        }
    };

    match source {
        None => seed_new_file(&path, &state_path, specs, &mut report)?,
        Some(source) => reconcile(&path, &state_path, &source, specs, forced, &mut report)?,
    }

    write_reference(dir, specs);
    Ok(report)
}

fn write_reference(dir: &Path, specs: &[MessageSpec]) {
    let path = dir.join(INJECTED_MESSAGES_REFERENCE_FILENAME);
    if let Err(e) = write_if_different(&path, &canonical_reference_bytes(specs)) {
        log::warn!(
            "[injected-messages] failed to write {}: {}",
            display_path(&path),
            e
        );
    }
}

fn seed_new_file(
    path: &Path,
    state_path: &Path,
    specs: &[MessageSpec],
    report: &mut ProvisionReport,
) -> Result<(), String> {
    let candidate = canonical_seed_bytes(specs);
    let intended: BTreeMap<String, String> = specs
        .iter()
        .map(|spec| (spec.id.to_string(), spec.default_template.to_string()))
        .collect();
    if let Err(reason) = verify_candidate(&candidate, &BTreeSet::new(), &intended) {
        report.writer_disabled = Some(reason.clone());
        log::warn!(
            "[injected-messages] refusing to seed {}: {}",
            display_path(path),
            reason
        );
        return Ok(());
    }

    if let Err(e) = publish(path, candidate.as_bytes()) {
        report.writer_disabled = Some(e.clone());
        log::warn!(
            "[injected-messages] failed to seed {}: {}",
            display_path(path),
            e
        );
        return Ok(());
    }

    // The sidecar is touched only after the main-file publish returns Ok. The
    // opposite order freezes an entry permanently on a plain failed write.
    let mut state = InjectedMessagesState {
        schema_version: SCHEMA_VERSION,
        messages: BTreeMap::new(),
    };
    for spec in specs {
        report
            .outcomes
            .insert(spec.id.to_string(), EntryOutcome::Seeded);
        state.messages.insert(
            spec.id.to_string(),
            MessageStateEntry {
                last_seeded_sha256: Some(sha256_hex(spec.default_template.as_bytes())),
                last_observed_sha256: None,
            },
        );
    }
    save_state(state_path, &state);
    Ok(())
}

fn save_state(path: &Path, state: &InjectedMessagesState) {
    let mut json = match serde_json::to_string_pretty(state) {
        Ok(json) => json,
        Err(e) => {
            log::warn!("[injected-messages] failed to serialize local state: {}", e);
            return;
        }
    };
    json.push('\n');
    if let Err(e) = write_if_different(path, &json) {
        log::warn!(
            "[injected-messages] failed to write {}: {}",
            display_path(path),
            e
        );
    }
}

/// One read, one publish. Every block rewrite and every append is applied to a
/// single in-memory string, which removes mixed-operation interleaving and
/// shrinks the cross-process window to one publish.
fn reconcile(
    path: &Path,
    state_path: &Path,
    source: &str,
    specs: &[MessageSpec],
    forced: Option<&BTreeSet<String>>,
    report: &mut ProvisionReport,
) -> Result<(), String> {
    let model = analyze_source(source);
    if let Some(reason) = model
        .strict_invalid
        .as_ref()
        .or(model.writer_blocked.as_ref())
    {
        report.writer_disabled = Some(reason.clone());
        log::warn!(
            "[injected-messages] {} is preserved untouched and the writer is disabled for this \
             run: {}",
            display_path(path),
            reason
        );
        return Ok(());
    }

    let mut state = load_state(state_path);
    let mut state_changed = false;
    let mut refresh: BTreeMap<String, &MessageSpec> = BTreeMap::new();
    let mut append: Vec<&MessageSpec> = Vec::new();

    for spec in specs {
        let is_forced = forced.is_some_and(|ids| ids.contains(spec.id));
        let current_hash = sha256_hex(spec.default_template.as_bytes());
        let entry_state = state.messages.entry(spec.id.to_string()).or_default();

        match model.entries.get(spec.id) {
            None => {
                append.push(spec);
                report
                    .outcomes
                    .insert(spec.id.to_string(), EntryOutcome::Appended);
            }
            Some(EntryValue::Malformed) => {
                // A non-string or absent `template` has no span a writer can
                // safely target, so not even a forced run rewrites it:
                // "repair by re-adding the missing template" would clobber a
                // deliberate deletion on the very next start.
                report
                    .outcomes
                    .insert(spec.id.to_string(), EntryOutcome::PreservedUserEdit);
                warn_once(
                    format!("entry:{}:malformed", spec.id),
                    &format!(
                        "[injected-messages] `{}` in {} has no usable `template`; it is left \
                         untouched and the built-in default is rendered",
                        spec.id,
                        display_path(path)
                    ),
                );
            }
            Some(EntryValue::Template(template)) => {
                let observed_hash = sha256_hex(template.as_bytes());
                let recognized = entry_state.last_seeded_sha256.as_deref() == Some(&observed_hash)
                    || spec
                        .known_default_sha256
                        .iter()
                        .any(|known| *known == observed_hash);

                if is_forced {
                    refresh.insert(spec.id.to_string(), spec);
                    report
                        .outcomes
                        .insert(spec.id.to_string(), EntryOutcome::Refreshed);
                } else if template.trim().is_empty() {
                    report
                        .outcomes
                        .insert(spec.id.to_string(), EntryOutcome::PreservedUserEdit);
                    warn_once(
                        format!("entry:{}:blank", spec.id),
                        &format!(
                            "[injected-messages] `{}` in {} is blank; it is left untouched and \
                             the built-in default is rendered",
                            spec.id,
                            display_path(path)
                        ),
                    );
                    if entry_state.last_observed_sha256.as_deref() != Some(&observed_hash) {
                        entry_state.last_observed_sha256 = Some(observed_hash);
                        state_changed = true;
                    }
                } else if template == spec.default_template {
                    report
                        .outcomes
                        .insert(spec.id.to_string(), EntryOutcome::AlreadyCurrent);
                    if entry_state.last_seeded_sha256.as_deref() != Some(&current_hash) {
                        entry_state.last_seeded_sha256 = Some(current_hash.clone());
                        state_changed = true;
                    }
                    if entry_state.last_observed_sha256.is_some() {
                        entry_state.last_observed_sha256 = None;
                        state_changed = true;
                    }
                } else if recognized {
                    refresh.insert(spec.id.to_string(), spec);
                    report
                        .outcomes
                        .insert(spec.id.to_string(), EntryOutcome::Refreshed);
                } else {
                    report
                        .outcomes
                        .insert(spec.id.to_string(), EntryOutcome::PreservedUserEdit);
                    warn_once(
                        format!("entry:{}:user-edit", spec.id),
                        &format!(
                            "[injected-messages] `{}` in {} was edited; it is kept as it is, and \
                             a newer built-in default is available",
                            spec.id,
                            display_path(path)
                        ),
                    );
                    if entry_state.last_observed_sha256.as_deref() != Some(&observed_hash) {
                        entry_state.last_observed_sha256 = Some(observed_hash);
                        state_changed = true;
                    }
                }
            }
        }
    }

    if !refresh.is_empty() || !append.is_empty() {
        if let Err(reason) =
            publish_edits(path, source, &model, &refresh, &append, forced.is_some())
        {
            report.writer_disabled = Some(reason.clone());
            log::warn!(
                "[injected-messages] {} is left untouched: {}",
                display_path(path),
                reason
            );
            // Nothing reached disk, so no outcome may claim it did, and the
            // sidecar must not be advanced either.
            for id in refresh.keys() {
                report
                    .outcomes
                    .insert(id.clone(), EntryOutcome::PreservedUserEdit);
            }
            for spec in &append {
                report.outcomes.remove(spec.id);
            }
            return Ok(());
        }
    }

    // The sidecar is advisory and is touched only after the main file is safely
    // on disk.
    for spec in specs {
        if refresh.contains_key(spec.id) || append.iter().any(|queued| queued.id == spec.id) {
            let entry_state = state.messages.entry(spec.id.to_string()).or_default();
            let current_hash = sha256_hex(spec.default_template.as_bytes());
            if entry_state.last_seeded_sha256.as_deref() != Some(&current_hash) {
                entry_state.last_seeded_sha256 = Some(current_hash);
                state_changed = true;
            }
            if entry_state.last_observed_sha256.is_some() {
                entry_state.last_observed_sha256 = None;
                state_changed = true;
            }
        }
    }
    if state.schema_version != SCHEMA_VERSION {
        state.schema_version = SCHEMA_VERSION;
        state_changed = true;
    }
    if state_changed {
        save_state(state_path, &state);
    }

    Ok(())
}

/// Builds the candidate, verifies it, and publishes exactly once.
fn publish_edits(
    path: &Path,
    source: &str,
    model: &ReadModel,
    refresh: &BTreeMap<String, &MessageSpec>,
    append: &[&MessageSpec],
    backup: bool,
) -> Result<(), String> {
    let crlf = source.contains("\r\n");
    let mut candidate = source.to_string();

    if !refresh.is_empty() {
        let doc: WriteDoc = toml::from_str(source)
            .map_err(|e| format!("its structure cannot be located for a write ({})", e))?;

        let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        for (id, spec) in refresh {
            let Some(entry) = doc.messages.get(id) else {
                return Err(format!("`{}` could not be located for a write", id));
            };
            let Some(spanned) = &entry.template else {
                return Err(format!("`{}` has no `template` value to rewrite", id));
            };
            if !spanned.get_ref().is_str() {
                return Err(format!("`{}` has a non-string `template`", id));
            }
            if !is_literal_quotable(spec.default_template) {
                return Err(format!(
                    "the built-in default for `{}` cannot be written as a TOML literal",
                    id
                ));
            }
            edits.push((spanned.span(), quote_literal(spec.default_template, crlf)));
        }

        // Descending, so an earlier edit never shifts a later span.
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.0.start));
        for (span, literal) in edits {
            if span.end > candidate.len() || !candidate.is_char_boundary(span.start) {
                return Err("a `template` span does not address the source".to_string());
            }
            candidate.replace_range(span, &literal);
        }
    }

    if !append.is_empty() {
        if !candidate.is_empty() && !candidate.ends_with('\n') {
            candidate.push('\n');
        }
        for spec in append {
            if !is_literal_quotable(spec.default_template) {
                return Err(format!(
                    "the built-in default for `{}` cannot be written as a TOML literal",
                    spec.id
                ));
            }
            let block = message_block(spec);
            candidate.push_str(&if crlf {
                block.replace('\n', "\r\n")
            } else {
                block
            });
        }
    }

    let previous_ids: BTreeSet<String> = model.entries.keys().cloned().collect();
    let mut intended: BTreeMap<String, String> = BTreeMap::new();
    for (id, spec) in refresh {
        intended.insert(id.clone(), spec.default_template.to_string());
    }
    for spec in append {
        intended.insert(spec.id.to_string(), spec.default_template.to_string());
    }
    verify_candidate(&candidate, &previous_ids, &intended)?;

    if backup {
        take_backup(path, source);
    }
    publish(path, candidate.as_bytes())
}

/// The writer never publishes text it has not re-parsed. Without this gate a
/// blind append onto a file whose `messages` is an inline table would make the
/// operator's own config invalid on the next start.
fn verify_candidate(
    candidate: &str,
    previous_ids: &BTreeSet<String>,
    intended: &BTreeMap<String, String>,
) -> Result<(), String> {
    let model = analyze_source(candidate);
    if let Some(reason) = model.strict_invalid {
        return Err(format!("the result would be invalid ({})", reason));
    }
    for id in previous_ids {
        if !model.entries.contains_key(id) {
            return Err(format!("the result would lose `{}`", id));
        }
    }
    for (id, expected) in intended {
        match model.entries.get(id) {
            Some(EntryValue::Template(template)) if template == expected => {}
            _ => return Err(format!("the result would not carry `{}` as intended", id)),
        }
    }
    Ok(())
}

fn take_backup(path: &Path, source: &str) {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| INJECTED_MESSAGES_FILENAME.to_string());
    name.push_str(&format!(".bak-{}", stamp));
    let backup: PathBuf = path.with_file_name(name);
    if let Err(e) = publish(&backup, source.as_bytes()) {
        log::warn!(
            "[injected-messages] failed to write backup {}: {}",
            display_path(&backup),
            e
        );
    }
}

// ---------------------------------------------------------------------------
// Reseed
// ---------------------------------------------------------------------------

pub(crate) fn known_message_ids() -> Vec<&'static str> {
    KNOWN_MESSAGES.iter().map(|spec| spec.id).collect()
}

/// Resets templates to the shipped defaults through the same surgical writer,
/// so comments, unknown keys, entry order and every other id's edits survive.
pub(crate) fn reseed(dir: &Path, target: ReseedTarget) -> Result<Vec<String>, String> {
    let ids: BTreeSet<String> = match &target {
        ReseedTarget::All => KNOWN_MESSAGES
            .iter()
            .map(|spec| spec.id.to_string())
            .collect(),
        ReseedTarget::Id(id) => {
            if spec_for(id).is_none() {
                return Err(format!(
                    "Unknown message id '{}'. Valid ids: {}",
                    id,
                    known_message_ids().join(", ")
                ));
            }
            BTreeSet::from([id.clone()])
        }
    };

    let report = provision(dir, &KNOWN_MESSAGES, Some(&ids))?;
    if let Some(reason) = report.writer_disabled {
        return Err(format!(
            "{} was left untouched: {}",
            display_path(&dir.join(INJECTED_MESSAGES_FILENAME)),
            reason
        ));
    }

    let mut reset: Vec<String> = Vec::new();
    for id in &ids {
        match report.outcomes.get(id) {
            Some(
                EntryOutcome::Refreshed
                | EntryOutcome::Seeded
                | EntryOutcome::Appended
                | EntryOutcome::AlreadyCurrent,
            ) => reset.push(id.clone()),
            // 4.8 forbids repairing a deliberately deleted `template`, and a
            // forced run must not become a back door around it. The log is the
            // only channel to the operator, so the message states both the
            // cause and the remedy rather than only that nothing changed.
            _ => {
                let path = display_path(&dir.join(INJECTED_MESSAGES_FILENAME));
                return Err(format!(
                    "`{id}` could not be reset and {path} was left untouched: its \
                     `[messages.{id}]` table exists but `template` is absent or is not a \
                     string, and re-adding it would clobber a deliberate deletion. Delete \
                     the whole `[messages.{id}]` entry and run this command again to \
                     rebuild it from the shipped default."
                ));
            }
        }
    }
    Ok(reset)
}

#[cfg(test)]
mod tests;
