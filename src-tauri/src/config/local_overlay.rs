//! #1737 - the `.local` alter-ego override layer.
//!
//! Two read-time overrides, one policy module:
//!
//! * Markdown: when `<name>.local.md` sits next to `<name>.md`, the session
//!   receives the local file's bytes instead of the base file's bytes.
//! * JSON: when `settings.local.json` sits next to `settings.json`, the two are
//!   deep-merged once at load, and the base values the overlay displaces are
//!   captured so every later save can restore them.
//!
//! **Leaf invariant, load-bearing.** Production code in this module references
//! nothing inside the crate and does not use `log`. It uses `std` and
//! `serde_json` only, and every list of crate-specific key names it needs is
//! passed in by the caller. Breaking the invariant would create the arc
//! `local_overlay -> settings`, which together with `settings -> local_overlay`
//! is a two-module cycle. Diagnostics are returned as typed values and rendered
//! by the caller (plan D22), which is also what makes them assertable without a
//! global logger.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The name of the JSON overlay file, read next to the base `settings.json`.
const SETTINGS_LOCAL_FILE_NAME: &str = "settings.local.json";

/// The suffix that turns `<name>.md` into its operator-owned override.
const MARKDOWN_LOCAL_SUFFIX: &str = ".local.md";

/// Why an overlay was not applied at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayRejection {
    Unreadable(String),
    InvalidJson(String),
    NotAnObject,
    MergedValueUndecodable(String),
}

impl OverlayRejection {
    fn describe(&self) -> String {
        match self {
            OverlayRejection::Unreadable(reason) => format!("could not be read ({reason})"),
            OverlayRejection::InvalidJson(reason) => format!("is not valid JSON ({reason})"),
            OverlayRejection::NotAnObject => "is not a JSON object".to_string(),
            OverlayRejection::MergedValueUndecodable(reason) => format!(
                "produced settings that could not be decoded ({reason}); using the base file alone"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayDiagnosticLevel {
    Error,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayDiagnostic {
    Rejected {
        source: String,
        rejection: OverlayRejection,
    },
    IneligibleKeyDropped {
        source: String,
        key: String,
        rule: &'static str,
    },
    Applied {
        source: String,
        owned: Vec<String>,
    },
    /// #1737 (D9) - a present `*.local.md` that could not be used. Lives on the
    /// same enum as the JSON records so both sides have one `level()`, one
    /// `render()`, and one three-line rendering match per source file.
    MarkdownRejected {
        source: String,
        reason: String,
    },
}

impl OverlayDiagnostic {
    pub(crate) fn level(&self) -> OverlayDiagnosticLevel {
        match self {
            OverlayDiagnostic::Rejected { .. }
            | OverlayDiagnostic::IneligibleKeyDropped { .. }
            | OverlayDiagnostic::MarkdownRejected { .. } => OverlayDiagnosticLevel::Error,
            OverlayDiagnostic::Applied { .. } => OverlayDiagnosticLevel::Info,
        }
    }

    pub(crate) fn render(&self) -> String {
        match self {
            OverlayDiagnostic::Rejected { source, rejection } => {
                format!("#1737: ignoring {source}: it {}", rejection.describe())
            }
            OverlayDiagnostic::IneligibleKeyDropped { source, key, rule } => {
                format!("#1737: dropped key `{key}` from {source}: it is not overridable ({rule})")
            }
            OverlayDiagnostic::Applied { source, owned } => {
                format!("#1737: applied {source}, overriding {}", owned.join(", "))
            }
            OverlayDiagnostic::MarkdownRejected { source, reason } => {
                format!("#1737: ignoring {source}: {reason}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkdownOverride {
    Absent,
    Present(String),
    Rejected { path: String, reason: String },
}

impl MarkdownOverride {
    /// #1737 (D9) - the typed diagnostic for a rejected override, `None` for
    /// `Absent` and `Present`. The caller renders it through the same
    /// `level()` / `render()` pair the JSON records use.
    pub(crate) fn diagnostic(&self) -> Option<OverlayDiagnostic> {
        match self {
            MarkdownOverride::Rejected { path, reason } => {
                Some(OverlayDiagnostic::MarkdownRejected {
                    source: path.clone(),
                    reason: reason.clone(),
                })
            }
            MarkdownOverride::Absent | MarkdownOverride::Present(_) => None,
        }
    }
}

/// `<dir>/<stem>.local.md` for a path whose extension is exactly `md`; `None` otherwise.
pub(crate) fn markdown_override_path(base: &Path) -> Option<PathBuf> {
    if base.extension()?.to_str()? != "md" {
        return None;
    }
    let stem = base.file_stem()?.to_str()?;
    Some(base.with_file_name(format!("{stem}{MARKDOWN_LOCAL_SUFFIX}")))
}

/// The `.local.md` override for `base`. Never logs; the caller renders
/// `diagnostic()`.
///
/// Applies the same file discipline as `read_context_template`: reject a
/// symlink or a non-regular file, then require UTF-8. An absent override is the
/// normal case and yields `Absent` with no diagnostic.
pub(crate) fn read_markdown_override(base: &Path) -> MarkdownOverride {
    let Some(path) = markdown_override_path(base) else {
        return MarkdownOverride::Absent;
    };
    let display = path.display().to_string();
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return MarkdownOverride::Absent,
        Err(e) => {
            return MarkdownOverride::Rejected {
                path: display,
                reason: format!("could not be inspected ({e})"),
            }
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return MarkdownOverride::Rejected {
            path: display,
            reason: "exists but is not a regular file".to_string(),
        };
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return MarkdownOverride::Rejected {
                path: display,
                reason: format!("could not be read ({e})"),
            }
        }
    };
    match String::from_utf8(bytes) {
        Ok(content) => MarkdownOverride::Present(content),
        Err(e) => MarkdownOverride::Rejected {
            path: display,
            reason: format!("is not valid UTF-8 ({e})"),
        },
    }
}

/// #1737 (D13) - a repair that creates one object per element id of a source
/// array. Structural, so `local_overlay` needs no crate knowledge: the caller
/// names the array key, the element field that identifies an element, and the
/// path prefix under which the repair creates one object per id.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DerivedIdClosure {
    pub(crate) source_key: &'static str,
    pub(crate) id_field: &'static str,
    pub(crate) derived_prefix: &'static [&'static str],
}

/// The rule string reported for a key dropped by the disk-authoritative table.
pub(crate) const RULE_DISK_AUTHORITATIVE: &str = "disk-authoritative (D7a)";

/// The rule string reported for a key dropped by the legacy migration-source table.
pub(crate) const RULE_LEGACY_MIGRATION_SOURCE: &str = "legacy migration source (D7b)";

/// The overlay actually applied, plus the base values it displaces.
///
/// `paths` and `base_values` are parallel and always the same length: entry `i`
/// says "the overlay owns `paths[i]`, and the base file held `base_values[i]`
/// there". `dropped` and `dropped_rules` are parallel in the same way.
#[derive(Debug, Default, Clone)]
pub(crate) struct LocalSettingsOverlay {
    paths: Vec<Vec<String>>,
    base_values: Vec<Option<Value>>,
    dropped: Vec<String>,
    dropped_rules: Vec<&'static str>,
    rejection: Option<OverlayRejection>,
}

impl LocalSettingsOverlay {
    /// True when no overlay is in force (absent, empty, or rejected).
    pub(crate) fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Owned JSON paths, deepest key last, in a deterministic order, for
    /// diagnostics and tests. Includes the D13 derived-id entries.
    pub(crate) fn owned_paths(&self) -> &[Vec<String>] {
        &self.paths
    }

    /// #1737 (D7c) - true when the restore plan contains any path whose first
    /// segment is `key`, i.e. THE RESTORE PLAN OWNS that top-level key. That is
    /// wider than "the overlay literally supplied a value at or below `key`",
    /// because the D13 derived-id closure also appends paths the overlay did not
    /// supply. The predicate the migration suppressions use.
    pub(crate) fn owns_top_level(&self, key: &str) -> bool {
        self.paths
            .iter()
            .any(|path| path.first().map(String::as_str) == Some(key))
    }

    /// `Some` when a present overlay file was not applied (D8, D21).
    pub(crate) fn rejection(&self) -> Option<&OverlayRejection> {
        self.rejection.as_ref()
    }

    /// Top-level keys removed by the ineligible tables, in table order.
    pub(crate) fn dropped_keys(&self) -> &[String] {
        &self.dropped
    }

    /// The full diagnostic record for this overlay. Empty for an absent overlay.
    pub(crate) fn diagnostics(&self, source: &str) -> Vec<OverlayDiagnostic> {
        let mut records = Vec::new();
        if let Some(rejection) = self.rejection() {
            records.push(OverlayDiagnostic::Rejected {
                source: source.to_string(),
                rejection: rejection.clone(),
            });
        }
        for (key, rule) in self.dropped_keys().iter().zip(self.dropped_rules.iter()) {
            records.push(OverlayDiagnostic::IneligibleKeyDropped {
                source: source.to_string(),
                key: key.clone(),
                rule,
            });
        }
        if !self.owned_paths().is_empty() {
            records.push(OverlayDiagnostic::Applied {
                source: source.to_string(),
                owned: self
                    .owned_paths()
                    .iter()
                    .map(|path| path.join("."))
                    .collect(),
            });
        }
        records
    }

    /// Reads `settings.local.json` next to `settings_path`, merges it into `base`
    /// in place, and records the restore plan. `ineligible_disk` and
    /// `ineligible_legacy` name top-level keys the overlay may not own (D7a, D7b);
    /// they are reported with different `rule` strings. `derived` is the D13
    /// closure table. Absent, unusable or empty overlays return a value for which
    /// `is_empty()` is true and leave `base` untouched.
    pub(crate) fn load_and_merge(
        settings_path: &Path,
        base: &mut Value,
        ineligible_disk: &[&str],
        ineligible_legacy: &[&str],
        derived: &[DerivedIdClosure],
    ) -> Self {
        let path = settings_path.with_file_name(SETTINGS_LOCAL_FILE_NAME);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => return Self::rejected(OverlayRejection::Unreadable(e.to_string())),
        };
        let value: Value = match serde_json::from_str(&contents) {
            Ok(value) => value,
            Err(e) => return Self::rejected(OverlayRejection::InvalidJson(e.to_string())),
        };
        let Value::Object(overlay) = value else {
            return Self::rejected(OverlayRejection::NotAnObject);
        };
        Self::from_overlay_object(base, overlay, ineligible_disk, ineligible_legacy, derived)
    }

    /// The pure core of `load_and_merge`, for tests and for callers that already
    /// hold the object.
    pub(crate) fn from_overlay_object(
        base: &mut Value,
        mut overlay: Map<String, Value>,
        ineligible_disk: &[&str],
        ineligible_legacy: &[&str],
        derived: &[DerivedIdClosure],
    ) -> Self {
        let mut state = Self::default();

        // The ineligible tables run FIRST: a removed key is neither applied nor
        // owned, it does not trigger the derived closure, and it is invisible to
        // `owns_top_level`.
        for (table, rule) in [
            (ineligible_disk, RULE_DISK_AUTHORITATIVE),
            (ineligible_legacy, RULE_LEGACY_MIGRATION_SOURCE),
        ] {
            for key in table {
                if overlay.remove(*key).is_some() {
                    state.dropped.push((*key).to_string());
                    state.dropped_rules.push(rule);
                }
            }
        }

        if overlay.is_empty() {
            return state;
        }

        // The restore plan is captured against the base BEFORE the merge.
        let mut path = Vec::new();
        let mut plan: Vec<(Vec<String>, Option<Value>)> = Vec::new();
        for (key, value) in &overlay {
            path.push(key.clone());
            collect(
                &mut path,
                base.as_object().and_then(|object| object.get(key)),
                value,
                &mut plan,
            );
            path.pop();
        }

        // D13 pre-merge halves: the base's id set and the base subtree under the
        // derived prefix, both read before the merge for the same reason `collect`
        // is.
        let pre_merge: Vec<(usize, Vec<String>, Option<Value>)> = derived
            .iter()
            .enumerate()
            .filter(|(_, closure)| overlay.contains_key(closure.source_key))
            .map(|(index, closure)| {
                let ids = element_ids(base.get(closure.source_key), closure.id_field);
                let subtree = value_at(base, closure.derived_prefix).cloned();
                (index, ids, subtree)
            })
            .collect();

        merge_value(base, &Value::Object(overlay));

        for (index, base_ids, base_subtree) in pre_merge {
            let closure = &derived[index];
            let merged_ids = element_ids(base.get(closure.source_key), closure.id_field);
            for id in merged_ids {
                if base_ids.contains(&id) {
                    continue;
                }
                let mut derived_path: Vec<String> = closure
                    .derived_prefix
                    .iter()
                    .map(|segment| (*segment).to_string())
                    .collect();
                derived_path.push(id);
                if plan.iter().any(|(existing, _)| *existing == derived_path) {
                    continue;
                }
                let captured = base_subtree
                    .as_ref()
                    .and_then(|subtree| subtree.get(derived_path.last().expect("pushed above")))
                    .cloned();
                plan.push((derived_path, captured));
            }
        }

        for (owned_path, base_value) in plan {
            state.paths.push(owned_path);
            state.base_values.push(base_value);
        }
        state
    }

    /// Restores every owned path in `out` to its captured base value. Creates
    /// nothing (D19).
    pub(crate) fn restore_base(&self, out: &mut Map<String, Value>) {
        for (path, base_value) in self.paths.iter().zip(self.base_values.iter()) {
            let Some((leaf, parents)) = path.split_last() else {
                continue;
            };
            let Some(parent) = walk_existing_object_mut(out, parents) else {
                continue;
            };
            match base_value {
                Some(value) => {
                    parent.insert(leaf.clone(), value.clone());
                }
                None => {
                    parent.remove(leaf);
                }
            }
        }
    }

    /// Makes `out` agree with `effective` at every owned path (D18, D19). A path
    /// present in `effective` is copied in, creating missing intermediate objects.
    /// A path ABSENT from `effective` - its leaf missing, or an intermediate
    /// segment missing or not an object - is REMOVED from `out`, because the live
    /// settings hold no value there and `restore_base` has already put the base
    /// value back. Serde omits every field whose `skip_serializing_if` fires, so
    /// "absent from `effective`" is the normal shape of an override that sets such
    /// a field to `None`; leaving `out` alone there would hand the caller the base
    /// value. Removal creates nothing: a missing or non-object intermediate in
    /// `out` is a no-op.
    pub(crate) fn reapply_from(
        &self,
        effective: &Map<String, Value>,
        out: &mut Map<String, Value>,
    ) {
        for path in &self.paths {
            let Some((leaf, parents)) = path.split_last() else {
                continue;
            };
            let live = walk_existing_object(effective, parents).and_then(|parent| parent.get(leaf));
            match live {
                Some(value) => {
                    let value = value.clone();
                    walk_creating_object_mut(out, parents).insert(leaf.clone(), value);
                }
                None => {
                    if let Some(parent) = walk_existing_object_mut(out, parents) {
                        parent.remove(leaf);
                    }
                }
            }
        }
    }

    /// Records `MergedValueUndecodable` on an overlay that was applied and then
    /// failed to decode (D21). Returns the empty overlay that must be used instead.
    pub(crate) fn into_undecodable(mut self, reason: String) -> Self {
        self.paths.clear();
        self.base_values.clear();
        self.rejection = Some(OverlayRejection::MergedValueUndecodable(reason));
        self
    }

    fn rejected(rejection: OverlayRejection) -> Self {
        Self {
            rejection: Some(rejection),
            ..Self::default()
        }
    }
}

/// U2 and U3: two objects recurse, everything else replaces. An array therefore
/// falls into the catch-all and replaces the base array whole, and `[]` yields an
/// empty list.
fn merge_value(dst: &mut Value, src: &Value) {
    match (dst, src) {
        (Value::Object(d), Value::Object(s)) => {
            for (key, value) in s {
                match d.get_mut(key) {
                    Some(slot) => merge_value(slot, value),
                    None => {
                        d.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (dst, src) => *dst = src.clone(),
    }
}

/// U4: recurse only while the overlay and the base are both objects at the same
/// path; otherwise the overlay owns that path wholesale and the base value there
/// (possibly absent, hence `Option`) is what must be written back.
fn collect(
    path: &mut Vec<String>,
    base: Option<&Value>,
    overlay: &Value,
    out: &mut Vec<(Vec<String>, Option<Value>)>,
) {
    match (overlay, base) {
        (Value::Object(o), Some(Value::Object(b))) => {
            for (key, value) in o {
                path.push(key.clone());
                collect(path, b.get(key), value, out);
                path.pop();
            }
        }
        _ => out.push((path.clone(), base.cloned())),
    }
}

/// The `id_field` string values of the object elements of the array at `value`,
/// in order and without duplicates. Empty when `value` is absent or is not an
/// array of objects; elements without a string `id_field` are skipped, matching
/// serde, which would fail to decode them anyway.
fn element_ids(value: Option<&Value>, id_field: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let Some(Value::Array(elements)) = value else {
        return ids;
    };
    for element in elements {
        let Some(id) = element
            .as_object()
            .and_then(|object| object.get(id_field))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// The value at `path` inside `value`, walking objects only.
fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.as_object()?.get(*segment)?;
    }
    Some(current)
}

fn walk_existing_object<'a>(
    root: &'a Map<String, Value>,
    path: &[String],
) -> Option<&'a Map<String, Value>> {
    let mut current = root;
    for segment in path {
        current = current.get(segment)?.as_object()?;
    }
    Some(current)
}

fn walk_existing_object_mut<'a>(
    root: &'a mut Map<String, Value>,
    path: &[String],
) -> Option<&'a mut Map<String, Value>> {
    let mut current = root;
    for segment in path {
        current = current.get_mut(segment)?.as_object_mut()?;
    }
    Some(current)
}

/// Walks `root` along `path`, creating a missing intermediate object and
/// replacing a non-object one, so a present effective value can always be
/// written (D19). Only `reapply_from`'s present-value arm uses this;
/// `restore_base` and the removal arm create nothing.
fn walk_creating_object_mut<'a>(
    root: &'a mut Map<String, Value>,
    path: &[String],
) -> &'a mut Map<String, Value> {
    let mut current = root;
    for segment in path {
        let slot = current
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if !slot.is_object() {
            *slot = Value::Object(Map::new());
        }
        current = slot.as_object_mut().expect("just made an object");
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn overlay_of(
        base: &mut Value,
        overlay: Value,
        derived: &[DerivedIdClosure],
    ) -> LocalSettingsOverlay {
        let Value::Object(map) = overlay else {
            panic!("overlay fixture must be an object");
        };
        LocalSettingsOverlay::from_overlay_object(base, map, &[], &[], derived)
    }

    fn plain(base: &mut Value, overlay: Value) -> LocalSettingsOverlay {
        overlay_of(base, overlay, &[])
    }

    const TEST_CLOSURES: &[DerivedIdClosure] = &[DerivedIdClosure {
        source_key: "agents",
        id_field: "id",
        derived_prefix: &["codingAgentProfiles", "profilesByAgent"],
    }];

    // L1
    #[test]
    fn nested_override_replaces_only_the_final_key() {
        let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
        assert_eq!(base["watchers"]["a"]["enabled"], json!(true));
        assert_eq!(base["watchers"]["a"]["intervalMs"], json!(50));
        assert!(!state.is_empty());
    }

    // L2
    #[test]
    fn an_array_replaces_the_base_array_whole() {
        let mut base = json!({"agents": [{"id": "a"}, {"id": "b"}]});
        plain(&mut base, json!({"agents": [{"id": "c"}]}));
        assert_eq!(base["agents"], json!([{"id": "c"}]));

        let mut base = json!({"agents": [{"id": "a"}]});
        plain(&mut base, json!({"agents": []}));
        assert_eq!(base["agents"], json!([]));
    }

    // L3
    #[test]
    fn an_overlay_key_absent_from_the_base_is_inserted() {
        let mut base = json!({"logLevel": "info"});
        plain(&mut base, json!({"newKey": 1}));
        assert_eq!(base["newKey"], json!(1));
        assert_eq!(base["logLevel"], json!("info"));
    }

    // L4
    #[test]
    fn an_overlay_object_over_a_base_scalar_replaces_it_wholesale() {
        let mut base = json!({"watchers": 7});
        let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
        assert_eq!(base["watchers"], json!({"a": {"intervalMs": 50}}));
        assert_eq!(state.owned_paths(), &[vec!["watchers".to_string()]]);
        assert_eq!(state.base_values, vec![Some(json!(7))]);
    }

    // L5
    #[test]
    fn the_restore_plan_for_a_nested_override_is_one_leaf_entry() {
        let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
        assert_eq!(
            state.owned_paths(),
            &[vec![
                "watchers".to_string(),
                "a".to_string(),
                "intervalMs".to_string()
            ]]
        );
        assert_eq!(state.base_values, vec![Some(json!(500))]);
    }

    // L6
    #[test]
    fn an_overlay_subtree_absent_from_the_base_captures_none() {
        let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
        assert_eq!(
            state.owned_paths(),
            &[vec!["watchers".to_string(), "z".to_string()]]
        );
        assert_eq!(state.base_values, vec![None]);
    }

    // L7
    #[test]
    fn restore_base_puts_the_base_leaf_back_and_leaves_siblings_alone() {
        let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
        let Value::Object(mut out) = base.clone() else {
            unreachable!()
        };
        out.insert("sibling".to_string(), json!(2));
        state.restore_base(&mut out);
        assert_eq!(out["watchers"]["a"]["intervalMs"], json!(500));
        assert_eq!(out["watchers"]["a"]["enabled"], json!(true));
        assert_eq!(out["sibling"], json!(2));
    }

    // L8
    #[test]
    fn restore_base_removes_only_the_owned_subtree_for_a_none_entry() {
        let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
        let Value::Object(mut out) = base.clone() else {
            unreachable!()
        };
        state.restore_base(&mut out);
        assert!(out["watchers"].get("z").is_none());
        assert_eq!(out["watchers"]["a"]["intervalMs"], json!(500));
    }

    // L9
    #[test]
    fn restore_base_over_a_missing_or_scalar_intermediate_is_a_no_op() {
        let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));

        let Value::Object(mut missing) = json!({"other": 1}) else {
            unreachable!()
        };
        state.restore_base(&mut missing);
        assert_eq!(Value::Object(missing), json!({"other": 1}));

        let Value::Object(mut scalar) = json!({"watchers": 5}) else {
            unreachable!()
        };
        state.restore_base(&mut scalar);
        assert_eq!(Value::Object(scalar), json!({"watchers": 5}));
    }

    // L10
    #[test]
    fn reapply_from_reproduces_the_effective_value_including_a_removed_path() {
        let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
        let Value::Object(effective) = base.clone() else {
            unreachable!()
        };
        let mut out = effective.clone();
        state.restore_base(&mut out);
        assert!(out["watchers"].get("z").is_none());
        state.reapply_from(&effective, &mut out);
        assert_eq!(out["watchers"]["z"], json!({"intervalMs": 9}));
    }

    // L11
    #[test]
    fn reapply_from_creates_a_missing_intermediate_object() {
        let mut base = json!({"watchers": {}});
        let state = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
        assert_eq!(
            state.owned_paths(),
            &[vec!["watchers".to_string(), "z".to_string()]]
        );
        let Value::Object(effective) = base.clone() else {
            unreachable!()
        };
        let mut out: Map<String, Value> = Map::new();
        state.reapply_from(&effective, &mut out);
        assert_eq!(out["watchers"]["z"], json!({"intervalMs": 9}));
    }

    // L12
    #[test]
    fn an_empty_overlay_object_owns_nothing() {
        let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
        let state = plain(&mut base, json!({}));
        assert!(state.is_empty());
        assert!(state.owned_paths().is_empty());
        assert_eq!(base, json!({"watchers": {"a": {"intervalMs": 500}}}));

        let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {}}));
        assert!(state.owned_paths().is_empty());
        assert_eq!(base, json!({"watchers": {"a": {"intervalMs": 500}}}));
    }

    // L13
    #[test]
    fn a_disk_ineligible_key_is_dropped_and_owns_nothing() {
        let mut base = json!({"rootToken": "base-token", "logLevel": "info"});
        let Value::Object(overlay) = json!({"rootToken": "override", "logLevel": "debug"}) else {
            unreachable!()
        };
        let state =
            LocalSettingsOverlay::from_overlay_object(&mut base, overlay, &["rootToken"], &[], &[]);
        assert_eq!(base["rootToken"], json!("base-token"));
        assert_eq!(base["logLevel"], json!("debug"));
        assert_eq!(state.dropped_keys(), &["rootToken".to_string()]);
        let records = state.diagnostics("settings.local.json");
        let dropped: Vec<&OverlayDiagnostic> = records
            .iter()
            .filter(|record| matches!(record, OverlayDiagnostic::IneligibleKeyDropped { .. }))
            .collect();
        assert_eq!(dropped.len(), 1);
        match dropped[0] {
            OverlayDiagnostic::IneligibleKeyDropped { key, rule, .. } => {
                assert_eq!(key, "rootToken");
                assert_eq!(*rule, RULE_DISK_AUTHORITATIVE);
            }
            _ => unreachable!(),
        }
    }

    // L14
    #[test]
    fn a_legacy_ineligible_key_is_dropped_with_the_legacy_rule() {
        let mut base = json!({"sidebarZoom": 1.5});
        let Value::Object(overlay) = json!({"sidebarZoom": 2.0}) else {
            unreachable!()
        };
        let state = LocalSettingsOverlay::from_overlay_object(
            &mut base,
            overlay,
            &[],
            &["sidebarZoom"],
            &[],
        );
        assert_eq!(base["sidebarZoom"], json!(1.5));
        assert!(state.is_empty());
        match &state.diagnostics("settings.local.json")[0] {
            OverlayDiagnostic::IneligibleKeyDropped { key, rule, .. } => {
                assert_eq!(key, "sidebarZoom");
                assert_eq!(*rule, RULE_LEGACY_MIGRATION_SOURCE);
            }
            other => panic!("unexpected record: {other:?}"),
        }
    }

    // L15
    #[test]
    fn invalid_json_is_rejected_and_leaves_the_base_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("settings.json");
        std::fs::write(temp.path().join("settings.local.json"), "{ not json").unwrap();
        let mut base = json!({"logLevel": "info"});
        let state = LocalSettingsOverlay::load_and_merge(&settings, &mut base, &[], &[], &[]);
        assert!(matches!(
            state.rejection(),
            Some(OverlayRejection::InvalidJson(_))
        ));
        assert!(state.is_empty());
        assert_eq!(base, json!({"logLevel": "info"}));
        let records = state.diagnostics("settings.local.json");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].level(), OverlayDiagnosticLevel::Error);
    }

    // L16
    #[test]
    fn a_top_level_non_object_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("settings.json");
        std::fs::write(temp.path().join("settings.local.json"), "[1, 2]").unwrap();
        let mut base = json!({"logLevel": "info"});
        let state = LocalSettingsOverlay::load_and_merge(&settings, &mut base, &[], &[], &[]);
        assert_eq!(state.rejection(), Some(&OverlayRejection::NotAnObject));
        assert!(state.is_empty());
        assert_eq!(base, json!({"logLevel": "info"}));
        assert_eq!(state.diagnostics("x").len(), 1);
    }

    // L17
    #[test]
    fn an_unreadable_overlay_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("settings.json");
        std::fs::create_dir(temp.path().join("settings.local.json")).unwrap();
        let mut base = json!({"logLevel": "info"});
        let state = LocalSettingsOverlay::load_and_merge(&settings, &mut base, &[], &[], &[]);
        assert!(matches!(
            state.rejection(),
            Some(OverlayRejection::Unreadable(_))
        ));
        assert_eq!(base, json!({"logLevel": "info"}));
    }

    // L18
    #[test]
    fn an_absent_overlay_file_produces_no_diagnostic_at_all() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("settings.json");
        let mut base = json!({"logLevel": "info"});
        let state = LocalSettingsOverlay::load_and_merge(&settings, &mut base, &[], &[], &[]);
        assert!(state.is_empty());
        assert_eq!(state.rejection(), None);
        assert!(state.diagnostics("settings.local.json").is_empty());
        assert!(state.owned_paths().is_empty());
        assert_eq!(base, json!({"logLevel": "info"}));
    }

    // L19
    #[test]
    fn an_applied_overlay_records_one_info_record_naming_every_owned_path() {
        let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
        let records = state.diagnostics("C:/x/settings.local.json");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].level(), OverlayDiagnosticLevel::Info);
        let rendered = records[0].render();
        assert!(rendered.contains("C:/x/settings.local.json"), "{rendered}");
        assert!(rendered.contains("watchers.a.intervalMs"), "{rendered}");
    }

    // L20
    #[test]
    fn the_derived_id_closure_introduces_exactly_the_new_ids() {
        let mut base = json!({"agents": [{"id": "a"}, {"id": "b"}]});
        let state = overlay_of(
            &mut base,
            json!({"agents": [{"id": "b"}, {"id": "c"}]}),
            TEST_CLOSURES,
        );
        let derived: Vec<&Vec<String>> = state
            .owned_paths()
            .iter()
            .filter(|path| path.first().map(String::as_str) == Some("codingAgentProfiles"))
            .collect();
        assert_eq!(
            derived,
            vec![&vec![
                "codingAgentProfiles".to_string(),
                "profilesByAgent".to_string(),
                "c".to_string()
            ]]
        );

        let mut base = json!({"agents": [{"id": "a"}]});
        let state = overlay_of(&mut base, json!({"logLevel": "debug"}), TEST_CLOSURES);
        assert_eq!(state.owned_paths(), &[vec!["logLevel".to_string()]]);
    }

    // L21
    #[test]
    fn the_closure_captures_the_base_value_and_never_duplicates_an_owned_path() {
        let mut base = json!({
            "agents": [{"id": "a"}],
            "codingAgentProfiles": {"profilesByAgent": {"stale": {"A": {"command": "x"}}}}
        });
        let state = overlay_of(
            &mut base,
            json!({"agents": [{"id": "stale"}, {"id": "fresh"}]}),
            TEST_CLOSURES,
        );
        let entries: Vec<(Vec<String>, Option<Value>)> = state
            .paths
            .iter()
            .cloned()
            .zip(state.base_values.iter().cloned())
            .filter(|(path, _)| path.first().map(String::as_str) == Some("codingAgentProfiles"))
            .collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, Some(json!({"A": {"command": "x"}})));
        assert_eq!(entries[1].1, None);

        // The operator already owns the exact derived path: no duplicate entry.
        let mut base = json!({"agents": [{"id": "a"}]});
        let state = overlay_of(
            &mut base,
            json!({
                "agents": [{"id": "fresh"}],
                "codingAgentProfiles": {"profilesByAgent": {"fresh": {"A": {}}}}
            }),
            TEST_CLOSURES,
        );
        let matching = state
            .owned_paths()
            .iter()
            .filter(|path| {
                **path
                    == vec![
                        "codingAgentProfiles".to_string(),
                        "profilesByAgent".to_string(),
                        "fresh".to_string(),
                    ]
            })
            .count();
        assert_eq!(matching, 1);
    }

    // L22
    #[test]
    fn an_ineligible_source_key_never_triggers_the_closure() {
        let mut base = json!({"agents": [{"id": "a"}]});
        let Value::Object(overlay) = json!({"agents": [{"id": "c"}]}) else {
            unreachable!()
        };
        let state = LocalSettingsOverlay::from_overlay_object(
            &mut base,
            overlay,
            &["agents"],
            &[],
            TEST_CLOSURES,
        );
        assert!(state.owned_paths().is_empty());
        assert_eq!(base["agents"], json!([{"id": "a"}]));
    }

    // L23
    #[test]
    fn markdown_override_path_appends_local_before_the_md_extension() {
        assert_eq!(
            markdown_override_path(Path::new("/x/Context.coordinator.md")),
            Some(PathBuf::from("/x/Context.coordinator.local.md"))
        );
        assert_eq!(
            markdown_override_path(Path::new("/x/a.b.md")),
            Some(PathBuf::from("/x/a.b.local.md"))
        );
        assert_eq!(markdown_override_path(Path::new("/x/Role.txt")), None);
        assert_eq!(markdown_override_path(Path::new("/x/noext")), None);
    }

    // L24
    #[test]
    fn read_markdown_override_maps_every_shape() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("Context.coordinator.md");
        std::fs::write(&base, "base bytes").unwrap();
        assert_eq!(read_markdown_override(&base), MarkdownOverride::Absent);

        let local = temp.path().join("Context.coordinator.local.md");
        std::fs::write(&local, "local bytes").unwrap();
        assert_eq!(
            read_markdown_override(&base),
            MarkdownOverride::Present("local bytes".to_string())
        );

        std::fs::remove_file(&local).unwrap();
        std::fs::create_dir(&local).unwrap();
        match read_markdown_override(&base) {
            MarkdownOverride::Rejected { reason, .. } => assert!(!reason.is_empty()),
            other => panic!("expected Rejected, got {other:?}"),
        }
        std::fs::remove_dir(&local).unwrap();

        std::fs::write(&local, [0x66u8, 0xff, 0x66]).unwrap();
        assert!(matches!(
            read_markdown_override(&base),
            MarkdownOverride::Rejected { .. }
        ));
    }

    // L25
    #[test]
    fn diagnostic_levels_are_error_for_three_variants_and_info_for_applied() {
        assert_eq!(
            OverlayDiagnostic::Rejected {
                source: "s".to_string(),
                rejection: OverlayRejection::NotAnObject,
            }
            .level(),
            OverlayDiagnosticLevel::Error
        );
        assert_eq!(
            OverlayDiagnostic::IneligibleKeyDropped {
                source: "s".to_string(),
                key: "rootToken".to_string(),
                rule: RULE_DISK_AUTHORITATIVE,
            }
            .level(),
            OverlayDiagnosticLevel::Error
        );
        assert_eq!(
            OverlayDiagnostic::MarkdownRejected {
                source: "s".to_string(),
                reason: "r".to_string(),
            }
            .level(),
            OverlayDiagnosticLevel::Error
        );
        assert_eq!(
            OverlayDiagnostic::Applied {
                source: "s".to_string(),
                owned: vec!["logLevel".to_string()],
            }
            .level(),
            OverlayDiagnosticLevel::Info
        );
    }

    // L26
    #[test]
    fn a_rejected_markdown_override_yields_an_error_diagnostic_naming_path_and_reason() {
        assert_eq!(MarkdownOverride::Absent.diagnostic(), None);
        assert_eq!(
            MarkdownOverride::Present("x".to_string()).diagnostic(),
            None
        );

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("Context.coordinator.md");
        std::fs::write(&base, "base bytes").unwrap();
        let local = temp.path().join("Context.coordinator.local.md");
        std::fs::create_dir(&local).unwrap();

        let value = read_markdown_override(&base);
        let diagnostic = value.diagnostic().expect("a rejection has a diagnostic");
        assert!(matches!(
            diagnostic,
            OverlayDiagnostic::MarkdownRejected { .. }
        ));
        assert_eq!(diagnostic.level(), OverlayDiagnosticLevel::Error);
        let rendered = diagnostic.render();
        assert!(
            rendered.contains(&local.display().to_string()),
            "{rendered}"
        );
        assert!(rendered.contains("regular file"), "{rendered}");
    }

    // L27
    #[test]
    fn owns_top_level_is_true_for_the_first_segment_of_every_owned_path() {
        let mut base = json!({"mainZoom": 1.5});
        let state = plain(&mut base, json!({"mainZoom": 1.0}));
        assert!(state.owns_top_level("mainZoom"));
        assert!(!state.owns_top_level("mainGeometry"));

        let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
        let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
        assert!(state.owns_top_level("watchers"));
        assert!(!state.owns_top_level("a"));
        assert!(!state.owns_top_level("intervalMs"));

        let mut base = json!({"agents": [{"id": "a"}]});
        let state = overlay_of(&mut base, json!({"agents": [{"id": "c"}]}), TEST_CLOSURES);
        assert!(state.owns_top_level("codingAgentProfiles"));
        assert!(state.owns_top_level("agents"));

        let default = LocalSettingsOverlay::default();
        for key in ["mainZoom", "watchers", "agents", "codingAgentProfiles"] {
            assert!(!default.owns_top_level(key));
        }
    }

    // L28
    #[test]
    fn a_dropped_key_is_not_owned() {
        let mut base = json!({"rootToken": "base"});
        let Value::Object(overlay) = json!({"rootToken": "override"}) else {
            unreachable!()
        };
        let state =
            LocalSettingsOverlay::from_overlay_object(&mut base, overlay, &["rootToken"], &[], &[]);
        assert!(!state.owns_top_level("rootToken"));
    }

    // L29
    #[test]
    fn a_non_object_array_or_non_array_source_key_yields_no_closure_entries() {
        let mut base = json!({"agents": [{"id": "a"}]});
        let state = overlay_of(&mut base, json!({"agents": [1, 2, "x"]}), TEST_CLOSURES);
        assert_eq!(state.owned_paths(), &[vec!["agents".to_string()]]);

        let mut base = json!({"agents": [{"id": "a"}]});
        let state = overlay_of(&mut base, json!({"agents": "not-an-array"}), TEST_CLOSURES);
        assert_eq!(state.owned_paths(), &[vec!["agents".to_string()]]);
    }

    // L30
    #[test]
    fn reapply_from_removes_an_owned_path_that_is_absent_from_effective() {
        let mut base = json!({"k": {"x": 1}, "sibling": 2});
        let state = plain(&mut base, json!({"k": null}));
        assert_eq!(state.owned_paths(), &[vec!["k".to_string()]]);
        assert_eq!(state.base_values, vec![Some(json!({"x": 1}))]);

        // `out` omits `k`, the shape serde produces for a `skip_serializing_if`
        // field whose value is `None`. `restore_base` puts the base object back.
        let Value::Object(mut out) = json!({"sibling": 2}) else {
            unreachable!()
        };
        state.restore_base(&mut out);
        assert_eq!(out["k"], json!({"x": 1}));

        // `effective` also omits `k`, so the live value is absence and the path
        // must be REMOVED, not left holding the base value.
        let Value::Object(effective) = json!({"sibling": 2}) else {
            unreachable!()
        };
        state.reapply_from(&effective, &mut out);
        assert!(out.get("k").is_none());
        assert_eq!(out["sibling"], json!(2));

        // A nested owned path is removed the same way.
        let mut base = json!({"watchers": {"a": {"commands": ["x"], "pattern": "p"}}});
        let nested = plain(&mut base, json!({"watchers": {"a": {"commands": null}}}));
        assert_eq!(
            nested.owned_paths(),
            &[vec![
                "watchers".to_string(),
                "a".to_string(),
                "commands".to_string()
            ]]
        );
        let Value::Object(mut out) = json!({"watchers": {"a": {"pattern": "p"}}}) else {
            unreachable!()
        };
        nested.restore_base(&mut out);
        assert_eq!(out["watchers"]["a"]["commands"], json!(["x"]));
        let Value::Object(effective) = json!({"watchers": {"a": {"pattern": "p"}}}) else {
            unreachable!()
        };
        nested.reapply_from(&effective, &mut out);
        assert!(out["watchers"]["a"].get("commands").is_none());
        assert_eq!(out["watchers"]["a"]["pattern"], json!("p"));

        // Removal creates nothing when `out`'s intermediate is missing or is not
        // an object, and does not panic.
        let Value::Object(mut missing) = json!({"other": 1}) else {
            unreachable!()
        };
        nested.reapply_from(&effective, &mut missing);
        assert_eq!(Value::Object(missing), json!({"other": 1}));
        let Value::Object(mut scalar) = json!({"watchers": 5}) else {
            unreachable!()
        };
        nested.reapply_from(&effective, &mut scalar);
        assert_eq!(Value::Object(scalar), json!({"watchers": 5}));

        // The second no-op shape: `restore_base` already removed the path
        // (captured base `None`) and `effective` does not carry it either.
        let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
        let absent = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
        let Value::Object(mut out) = json!({"watchers": {"a": {"intervalMs": 500}}}) else {
            unreachable!()
        };
        absent.restore_base(&mut out);
        let Value::Object(effective) = json!({"watchers": {"a": {"intervalMs": 500}}}) else {
            unreachable!()
        };
        absent.reapply_from(&effective, &mut out);
        assert!(out["watchers"].get("z").is_none());

        // And the value is still COPIED when `effective` does carry it, so the
        // removal rule cannot be satisfied by removing unconditionally.
        let Value::Object(effective) =
            json!({"watchers": {"a": {"intervalMs": 500}, "z": {"intervalMs": 9}}})
        else {
            unreachable!()
        };
        absent.reapply_from(&effective, &mut out);
        assert_eq!(out["watchers"]["z"], json!({"intervalMs": 9}));
    }

    #[test]
    fn into_undecodable_empties_the_plan_and_records_the_rejection() {
        let mut base = json!({"mainZoom": 1.5});
        let state = plain(&mut base, json!({"mainZoom": 1.0}));
        assert!(!state.is_empty());
        let state = state.into_undecodable("bad type".to_string());
        assert!(state.is_empty());
        assert!(state.owned_paths().is_empty());
        assert!(matches!(
            state.rejection(),
            Some(OverlayRejection::MergedValueUndecodable(_))
        ));
        let records = state.diagnostics("settings.local.json");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].level(), OverlayDiagnosticLevel::Error);
        assert!(records[0].render().contains("bad type"));
    }
}
