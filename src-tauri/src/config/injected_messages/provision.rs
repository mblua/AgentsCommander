use super::analyze_source;
use super::canonical_reference_bytes;
use super::canonical_seed_bytes;
use super::display_path;
use super::is_literal_quotable;
use super::message_block;
use super::quote_literal;
use super::render::spec_for;
use super::sha256_hex;
use super::warn_once;
use super::EntryOutcome;
use super::EntryValue;
use super::InjectedMessagesState;
use super::MessageSpec;
use super::MessageStateEntry;
use super::ProvisionReport;
use super::ReadModel;
use super::ReseedTarget;
use super::WriteDoc;
use super::INJECTED_MESSAGES_FILENAME;
use super::INJECTED_MESSAGES_REFERENCE_FILENAME;
use super::INJECTED_MESSAGES_STATE_FILENAME;
use super::KNOWN_MESSAGES;
use super::SCHEMA_VERSION;
use crate::config::local_config_io::write_file_atomic;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use crate::filesystem::injected_documents;

// ---------------------------------------------------------------------------
// Provisioning
// ---------------------------------------------------------------------------

pub(super) fn read_source(path: &Path) -> Result<Option<String>, String> {
    match injected_documents::source_metadata(path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err("the path is not a regular file".to_string());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("it cannot be inspected: {}", e)),
    }

    let bytes = injected_documents::read_source_bytes(path)
        .map_err(|e| format!("it cannot be read: {}", e))?;
    match String::from_utf8(bytes) {
        Ok(source) => Ok(Some(source)),
        Err(e) => Err(format!("it is not valid UTF-8: {}", e)),
    }
}

pub(super) fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
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
    match injected_documents::read_state_text(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => InjectedMessagesState::default(),
    }
}

/// Write-if-different. An unconditional write would mean an atomic publish on
/// every launch forever: mtime churn, pointless fsyncs, extra sharing-violation
/// surface, and a warning on every launch in a read-only config directory.
fn write_if_different(path: &Path, contents: &str) -> Result<bool, String> {
    if let Ok(existing) = injected_documents::read_source_bytes(path) {
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
    super::record_write(path);
    write_file_atomic(path, bytes)
}

pub(crate) fn ensure_injected_messages(
    dir: &Path,
    warnings: &crate::config::injected_messages::MessageWarnings,
) -> Result<ProvisionReport, String> {
    provision(dir, &KNOWN_MESSAGES, None, warnings)
}

/// Seeds, reconciles and appends, then regenerates the companion.
///
/// `forced` marks ids that must be rewritten to the current default regardless
/// of their content, which is what `reseed` is; a forced run takes a timestamped
/// backup before publishing, because it can overwrite operator text. Refresh
/// deliberately takes none: it only ever fires on content whose hash is provably
/// a shipped default, so there is no operator text to lose.
pub(super) fn provision(
    dir: &Path,
    specs: &[MessageSpec],
    forced: Option<&BTreeSet<String>>,
    warnings: &crate::config::injected_messages::MessageWarnings,
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
        Some(source) => reconcile(
            &path,
            &state_path,
            &source,
            specs,
            forced,
            &mut report,
            warnings,
        )?,
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

struct ReconcilePlan<'a> {
    state_changed: bool,
    refresh: BTreeMap<String, &'a MessageSpec>,
    append: Vec<&'a MessageSpec>,
}

fn recognizes_template(
    spec: &MessageSpec,
    entry_state: &MessageStateEntry,
    observed_hash: &str,
) -> bool {
    entry_state.last_seeded_sha256.as_deref() == Some(observed_hash)
        || spec
            .known_default_sha256
            .iter()
            .any(|known| *known == observed_hash)
}

/// Plan entry changes and advisory state updates before publishing any edits.
fn reconcile_entries<'a>(
    path: &Path,
    model: &ReadModel,
    specs: &'a [MessageSpec],
    forced: Option<&BTreeSet<String>>,
    state: &mut InjectedMessagesState,
    report: &mut ProvisionReport,
    warnings: &crate::config::injected_messages::MessageWarnings,
) -> ReconcilePlan<'a> {
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
                    warnings,
                );
            }
            Some(EntryValue::Template(template)) => {
                let observed_hash = sha256_hex(template.as_bytes());
                let recognized = recognizes_template(spec, entry_state, &observed_hash);

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
                        warnings,
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
                        warnings,
                    );
                    if entry_state.last_observed_sha256.as_deref() != Some(&observed_hash) {
                        entry_state.last_observed_sha256 = Some(observed_hash);
                        state_changed = true;
                    }
                }
            }
        }
    }

    ReconcilePlan {
        state_changed,
        refresh,
        append,
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
    warnings: &crate::config::injected_messages::MessageWarnings,
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
    let ReconcilePlan {
        mut state_changed,
        refresh,
        append,
    } = reconcile_entries(path, &model, specs, forced, &mut state, report, warnings);

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
pub(super) fn verify_candidate(
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
pub(crate) fn reseed(
    dir: &Path,
    target: ReseedTarget,
    warnings: &crate::config::injected_messages::MessageWarnings,
) -> Result<Vec<String>, String> {
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

    let report = provision(dir, &KNOWN_MESSAGES, Some(&ids), warnings)?;
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
