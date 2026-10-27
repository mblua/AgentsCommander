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

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod provision;
mod render;

pub(crate) use provision::ensure_injected_messages;
#[cfg(all(test, windows))]
use provision::is_link_or_reparse;
pub(crate) use provision::known_message_ids;
use provision::read_source;
pub(crate) use provision::reseed;
#[cfg(test)]
use provision::{provision, verify_candidate};
pub(crate) use render::render;
#[cfg(test)]
pub(crate) use render::render_with_template;
use render::spec_for;
#[cfg(test)]
use render::{expand_tokens, render_resolved, sanitize};
#[cfg(test)]
use tests::record_write;

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
// Registry load
// ---------------------------------------------------------------------------

/// Loaded once per process and never reloaded, so editing the file takes effect
/// on the next AC start, consistent with `settings.json`.
///
/// Under `cfg(test)` this is always empty, which makes the exact-bytes
/// assertions in `phone::mailbox` hermetic by construction rather than by luck:
/// under `cargo test` `config_dir()` resolves to a real directory next to the
/// test binary, and any file left there would make them order-dependent.
pub(crate) fn registry(_warnings: &MessageWarnings) -> &'static BTreeMap<String, String> {
    static REGISTRY: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        #[cfg(test)]
        {
            BTreeMap::new()
        }
        #[cfg(not(test))]
        {
            match crate::config::config_dir() {
                Some(dir) => load_registry_from_dir(&dir, _warnings),
                None => BTreeMap::new(),
            }
        }
    })
}

/// The registry load with no global state, so resolution is testable.
fn load_registry_from_dir(dir: &Path, warnings: &MessageWarnings) -> BTreeMap<String, String> {
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
                warnings,
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
            warnings,
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

pub struct MessageWarnings {
    warned: OnceLock<Mutex<HashSet<String>>>,
}

#[allow(clippy::new_without_default)]
impl MessageWarnings {
    pub fn new() -> Self {
        Self {
            warned: OnceLock::new(),
        }
    }

    /// Returns whether it emitted: `true` on the first use of `key`, `false`
    /// afterwards. That return value is the only observable this mechanism has, and
    /// the log is the only channel this feature has to the operator, so without it
    /// nothing about that channel is testable.
    fn warn_once(&self, key: String, message: &str) -> bool {
        let warned = self.warned.get_or_init(|| Mutex::new(HashSet::new()));
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
}

fn warn_once(key: String, message: &str, warnings: &MessageWarnings) -> bool {
    warnings.warn_once(key, message)
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

#[cfg(test)]
mod tests;
