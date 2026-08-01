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
pub(crate) const COVERAGE_VERSION: u32 = 1;

pub(crate) const CONTEXT_ALERT_MESSAGE_ID: &str = "context-alert";

pub(crate) const TOKEN_MEMBER: &str = "%MEMBER%";
pub(crate) const TOKEN_WORKGROUP: &str = "%WORKGROUP%";
pub(crate) const TOKEN_THRESHOLDS: &str = "%THRESHOLDS%";
pub(crate) const TOKEN_OBSERVED: &str = "%OBSERVED%";

/// The only embedded copy of the context-alert wording. 125 characters, 125
/// UTF-8 bytes, sha256 `e672581d47e7e4a4749b510f23eff72982ff3fa5261109122b3bdf8fdfda153f`.
pub(crate) const DEFAULT_CONTEXT_ALERT_TEMPLATE: &str = "[AC context alert] `%MEMBER%` in `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No action taken; you decide any follow-up.";

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
# Injected into the coordinator's terminal when a member crosses a configured
# context-usage threshold.
# Placeholders:
#   %MEMBER%      name of the observed member, e.g. dev-rust
#   %WORKGROUP%   workgroup name, e.g. wg-2-dev-team
#   %THRESHOLDS%  thresholds just crossed, already formatted, e.g. 50%, 75%
#   %OBSERVED%    observed context use, e.g. 91% (best-effort human signal)";

const KNOWN_MESSAGES: [MessageSpec; 1] = [MessageSpec {
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
}];

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
                    "[injected-messages] the `{}` template renders to nothing after sanitization; the built-in default is used instead",
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
                "[injected-messages] {} is invalid ({}); the file is preserved untouched and built-in defaults are rendered",
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
                "[injected-messages] {} is unusable ({}); it is left untouched and built-in defaults are rendered",
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
            "[injected-messages] {} is preserved untouched and the writer is disabled for this run: {}",
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
                        "[injected-messages] `{}` in {} has no usable `template`; it is left untouched and the built-in default is rendered",
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
                            "[injected-messages] `{}` in {} is blank; it is left untouched and the built-in default is rendered",
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
                            "[injected-messages] `{}` in {} was edited; it is kept as it is, and a newer built-in default is available",
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
        edits.sort_by(|left, right| right.0.start.cmp(&left.0.start));
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
mod tests {
    use super::*;
    use crate::pty::inject::validate_pty_input_text;
    use std::sync::atomic::{AtomicBool, Ordering};

    // ---- fixtures ---------------------------------------------------------

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
    const EXPECTED_SEED: &str = r##"# AgentsCommander injected PTY message templates.
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
coverage_version = 1

[messages.context-alert]
# Injected into the coordinator's terminal when a member crosses a configured
# context-usage threshold.
# Placeholders:
#   %MEMBER%      name of the observed member, e.g. dev-rust
#   %WORKGROUP%   workgroup name, e.g. wg-2-dev-team
#   %THRESHOLDS%  thresholds just crossed, already formatted, e.g. 50%, 75%
#   %OBSERVED%    observed context use, e.g. 91% (best-effort human signal)
template = '''
[AC context alert] `%MEMBER%` in `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No action taken; you decide any follow-up.
'''
"##;

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
            "[AC context alert] `%MEMBER%` in `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No action taken; you decide any follow-up."
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
        let template = "# Heading\n\n**bold** and `code` and *stars*\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```\n\n> quote %MEMBER%\n\n| a | b |\n|---|---|\n| 1 | 2 |";
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
        let expanded =
            render_with_template("%OBSERVED%%%AC_RESPONSE::abc::START%%", &alert_values());
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
        assert_eq!(EXPECTED_SEED.len(), 1533, "the pinned seed is 1533 bytes");
        assert!(!EXPECTED_SEED.contains('\r'), "the pinned seed is LF");
        let written = read(&main_path(dir.path()));
        assert_eq!(written, EXPECTED_SEED);
        assert!(!written.contains("\r\n"), "seeded file must be LF");
        assert!(written.ends_with("'''\n"));
        assert!(!written.ends_with("\n\n"), "exactly one trailing newline");
        assert!(written.contains("schema_version = 1"));
        assert!(written.contains("coverage_version = 1"));
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
            "# lead comment\nunknown_top = 7\nschema_version = 1\n\n[messages.zzz-other]\ntemplate = '''\nzzz\n'''\n\n[messages.context-alert]\n# doc comment\nunknown_inside = true\ntemplate = '''\n{}\n'''\n# trailing comment\n",
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
            "# operator comment\nschema_version = 1\n\n[messages.context-alert]\ntemplate = '''\n{}\n'''\n# a trailing comment with no newline after it",
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
        let source = "schema_version = 2\ncoverage_version = 1\n\n[messages.context-alert]\ntemplate = '''\nOPERATOR TEXT %MEMBER%\n'''\n";
        std::fs::write(main_path(dir.path()), source).expect("write");

        let report = ensure_injected_messages(dir.path()).expect("provision");
        assert!(report.writer_disabled.is_some());
        assert_eq!(read(&main_path(dir.path())), source);
        // Structure unknown: fall back to the built-in defaults.
        assert!(load_registry_from_dir(dir.path()).is_empty());
    }

    #[test]
    fn newer_coverage_version_keeps_rendering_from_file() {
        let dir = tempdir();
        let source = "schema_version = 1\ncoverage_version = 2\n\n[messages.context-alert]\ntemplate = '''\nOPERATOR TEXT %MEMBER%\n'''\n";
        std::fs::write(main_path(dir.path()), source).expect("write");

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
        let inside = "schema_version = 1\n\n[messages.context-alert]\ntemplate = '''\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> other\n'''\n";
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
        let source = "schema_version = 1\nunknown_top = \"x\"\n\n[messages.not-a-known-id]\ntemplate = '''\nsomething\n'''\n\n[messages.context-alert]\nunknown_inside = 1\ntemplate = '''\nOPERATOR %MEMBER%\n'''\n";
        std::fs::write(main_path(dir.path()), source).expect("write");

        let report = ensure_injected_messages(dir.path()).expect("provision");
        assert_eq!(report.writer_disabled, None);
        assert_eq!(read(&main_path(dir.path())), source);
        assert_eq!(
            load_registry_from_dir(dir.path()).get(CONTEXT_ALERT_MESSAGE_ID),
            Some(&"OPERATOR %MEMBER%".to_string())
        );
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
            "[AC context alert] `dev-rust` in `wg-2-dev-team` reached threshold(s): 50%, 75%, 90%. No action taken; you decide any follow-up."
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
        assert_eq!(reset, vec![CONTEXT_ALERT_MESSAGE_ID.to_string()]);
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
            "\n[AC context alert] `dev-rust` in `wg-2-dev-team` reached threshold(s): 50%, 75%, 90%. No action taken; you decide any follow-up.\n\r"
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
            "schema_version = 1\n\n[messages.context-alert]\n# the operator deleted the template line\n\n[messages.{}]\ntemplate = 42\n\n[messages.healthy]\ntemplate = '''\nHEALTHY\n'''\n",
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
        assert_eq!(read(&main_path(dir.path())), table_valued);
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
        let source = "schema_version = 1\n\n[messages.context-alert]\ntemplate = 'a'\n\n[messages.context-alert]\ntemplate = 'b'\n";
        let model = analyze_source(source);
        assert!(
            model.strict_invalid.is_some(),
            "a duplicate header must never become a silent last-wins"
        );
    }

    #[test]
    fn append_into_inline_table_file_publishes_nothing() {
        let dir = tempdir();
        let source =
            "schema_version = 1\nmessages = { \"context-alert\" = { template = \"X\" } }\n";
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
        let intended =
            BTreeMap::from([(CONTEXT_ALERT_MESSAGE_ID.to_string(), "value".to_string())]);
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
        let source = "schema_version = 1\n\n[messages.healthy]\ntemplate = '''\nHEALTHY\n'''\n\n[messages]\ncontext-alert = \"my own text\"\n";
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
        assert_eq!(read(&main_path(dir.path())), source);
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
        assert!(
            validate_pty_input_text(&render(CONTEXT_ALERT_MESSAGE_ID, &alert_values())).is_ok()
        );

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
            let rendered =
                render_resolved(CONTEXT_ALERT_MESSAGE_ID, template, true, &alert_values());
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
        let source = "schema_version = 1\n\n[messages.context-alert]\ntemplate = '''\nAlerta\n=======\ncuerpo del mensaje\n'''\n";
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
        assert_eq!(read(&main_path(dir.path())), source);

        // A non-integer version key is strict-invalid, unlike an absent one.
        assert!(analyze_source("schema_version = \"1\"\n")
            .strict_invalid
            .is_some());
        assert!(analyze_source("coverage_version = true\n")
            .strict_invalid
            .is_some());
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
}
