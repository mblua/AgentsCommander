// Callers of `ensure_claude_md_excludes` and `ensure_rtk_pretool_hook` (must be
// kept in sync with any new agent-creation flow — see issue #84 for the original
// `ensure_claude_md_excludes` miss and issue #120 for the rtk extension):
//   - commands/agent_creator.rs::write_claude_settings_local (Tauri cmd; frontend: NewAgentModal.tsx + SessionItem.tsx ctx-menu)
//   - cli/create_agent.rs (CLI `create-agent --launch <id>`)
//   - commands/entity_creation.rs::create_agent_matrix
//   - commands/entity_creation.rs::create_workgroup (per-replica)
//
// The retroactive sweep (issue #120) is `commands/config.rs::sweep_rtk_hook`,
// which reuses `enumerate_managed_agent_dirs` below.

/// Immutable substring embedded in every AC-injected rewriter command.
/// OFF-sweep removes any PreToolUse hook whose `command` contains this
/// string OR any marker in `RTK_LEGACY_MARKERS`. ON-sweep skips insertion
/// if any current-marker-bearing entry already exists (preserves user
/// customizations of the rewriter body across AC upgrades) AND cleans
/// legacy-marker entries in a pre-pass.
///
/// **Bumped from v1 to v2** because Claude Code's hook output schema
/// changed: v1 emitted `{decision:'modify', tool_input:{...}}` which
/// current Claude Code rejects with "Hook JSON output validation
/// failed". v2 uses `hookSpecificOutput.updatedInput` (see
/// `RTK_REWRITER_COMMAND`). The marker bump triggers automatic cleanup
/// of any v1 entries left over from earlier AC builds — see
/// `RTK_LEGACY_MARKERS` and `merge_rtk_hook`'s legacy pre-pass.
pub const RTK_HOOK_MARKER: &str = "@ac-rtk-marker-v2";

/// Legacy markers we still recognize for cleanup purposes. ON-sweep
/// removes legacy-marker entries before its idempotency check; OFF-sweep
/// removes both current AND legacy entries. Order does not matter; new
/// retirements are appended.
pub const RTK_LEGACY_MARKERS: &[&str] = &["@ac-rtk-marker-v1"];

/// Canonical RTK PreToolUse rewriter command. The leading
/// `'@ac-rtk-marker-v2';` is a JS string-literal expression statement —
/// node treats it as a no-op (string in statement position). The marker
/// is never executed and never affects rewriter behavior; it exists
/// solely to identify "this hook is AC-injected" across AC upgrades
/// (see `RTK_HOOK_MARKER`).
///
/// **Output schema (v2).** Emits
/// `{hookSpecificOutput:{hookEventName:'PreToolUse', updatedInput:{...}}}`
/// — the format Claude Code current accepts. v1 emitted a
/// `decision:'modify'` shape that the current validator rejects;
/// entries with the v1 marker are auto-cleaned by ON-sweep / OFF-sweep
/// (see `RTK_LEGACY_MARKERS`).
///
/// Mirrors `repo-AgentsCommander/.claude/settings.json` (project-level
/// hook). Must stay byte-identical to that file; the source-of-truth
/// test in this module loads the source `.claude/settings.json` at test
/// time and asserts equality.
pub const RTK_REWRITER_COMMAND: &str = r#"node -e "'@ac-rtk-marker-v2';const s=JSON.parse(require('fs').readFileSync(0,'utf8'));const c=s?.tool_input?.command;if(!c){process.exit(0)}if(/^rtk\s/.test(c)||/&&\s*rtk\s/.test(c)){process.exit(0)}const skip=/^(cd |mkdir |echo |cat <<|source |export |\.|set )/.test(c);if(skip){process.exit(0)}const parts=c.split(/\s*(&&|\|\||;)\s*/);const out=parts.map((p,i)=>{if(i%2===1)return p;if(/^rtk\s/.test(p))return p;return 'rtk '+p}).join(' ');if(out!==c){console.log(JSON.stringify({hookSpecificOutput:{hookEventName:'PreToolUse',updatedInput:{...s.tool_input,command:out}}}))}else{process.exit(0)}""#;

use std::path::Path;

/// Ensures `.claude/settings.local.json` in `dir` contains a `claudeMdExcludes`
/// entry pointing to `~/.claude/CLAUDE.md`.
///
/// - Creates `.claude/` if missing.
/// - Merges into an existing file (preserves other keys).
/// - Resolves the home directory dynamically.
pub fn ensure_claude_md_excludes(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", dir.display()));
    }

    let claude_dir = dir.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| format!("Failed to create .claude directory: {}", e))?;

    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let home_str = home.to_string_lossy().replace('\\', "/");
    let exclude_path = format!("{}/.claude/CLAUDE.md", home_str);

    let settings_path = claude_dir.join("settings.local.json");

    // Merge into existing file if present; treat parse errors and non-objects as empty
    let mut obj = if settings_path.exists() {
        let existing = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("Failed to read existing settings.local.json: {}", e))?;
        match serde_json::from_str::<serde_json::Value>(&existing) {
            Ok(v) if v.is_object() => v,
            _ => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };

    // guaranteed to be an object by the match above
    let map = obj.as_object_mut().unwrap();

    // Preserve existing array entries, just ensure our path is present
    let mut excludes: Vec<serde_json::Value> = map
        .get("claudeMdExcludes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let exclude_val = serde_json::Value::String(exclude_path.clone());
    if !excludes.contains(&exclude_val) {
        excludes.push(exclude_val);
    }

    map.insert(
        "claudeMdExcludes".to_string(),
        serde_json::Value::Array(excludes),
    );

    let content = serde_json::to_string_pretty(&obj)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&settings_path, format!("{}\n", content))
        .map_err(|e| format!("Failed to write settings.local.json: {}", e))?;

    Ok(())
}

/// Merges (`enabled=true`) or removes (`enabled=false`) the RTK PreToolUse
/// rewriter hook in `<dir>/.claude/settings.local.json`. Issue #120.
///
/// Non-destructive on every malformed input: bails with `log::warn!` and
/// returns `Ok(())` without modifying the file. UTF-8 BOM is stripped on
/// the read path. Idempotency and removal both filter by marker substring
/// (`RTK_HOOK_MARKER`), not byte-equality of the full command — this
/// preserves user customizations of the rewriter body across AC upgrades.
pub fn ensure_rtk_pretool_hook(dir: &Path, enabled: bool) -> Result<(), String> {
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", dir.display()));
    }

    let claude_dir = dir.join(".claude");
    let settings_path = claude_dir.join("settings.local.json");

    // OFF-path early exit: nothing to remove if file is missing.
    if !enabled && !settings_path.exists() {
        return Ok(());
    }

    // Read + BOM-strip + parse. On any parse failure or non-object root,
    // log + bail without modifying the file (grinch H1, M11).
    let mut obj: serde_json::Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("Failed to read existing settings.local.json: {}", e))?;
        let cleaned = raw.strip_prefix('\u{feff}').unwrap_or(raw.as_str());
        match serde_json::from_str::<serde_json::Value>(cleaned) {
            Ok(v) if v.is_object() => v,
            _ => {
                log::warn!(
                    "[rtk] Skipping {} for {}: file is not a JSON object (preserved as-is)",
                    if enabled { "ON-sweep" } else { "OFF-sweep" },
                    settings_path.display()
                );
                return Ok(());
            }
        }
    } else {
        // ON-path with missing file — start with an empty doc.
        serde_json::json!({})
    };

    // For ON-path, ensure .claude/ exists before we eventually write back.
    if enabled && !claude_dir.exists() {
        std::fs::create_dir_all(&claude_dir)
            .map_err(|e| format!("Failed to create .claude directory: {}", e))?;
    }

    // Mutate. Both helpers return `true` if `obj` was changed (caller should
    // write back) and `false` if the call was a structural no-op or a
    // wrong-shape bail (caller must NOT write — keep user's file untouched).
    let mutated = if enabled {
        merge_rtk_hook(&mut obj, &settings_path)
    } else {
        remove_rtk_hook(&mut obj, &settings_path)
    };

    if !mutated {
        return Ok(());
    }

    let content = serde_json::to_string_pretty(&obj)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&settings_path, format!("{}\n", content))
        .map_err(|e| format!("Failed to write settings.local.json: {}", e))?;

    Ok(())
}

/// Idempotent merge by marker (preserves user customizations of the
/// rewriter body across AC upgrades — see grinch M10). Returns `true` iff
/// `obj` was modified (caller should write).
///
/// Bails with `log::warn!` and returns `false` on any wrong-shape value
/// (grinch H2). The user's file is preserved verbatim by the outer caller.
fn merge_rtk_hook(obj: &mut serde_json::Value, settings_path: &Path) -> bool {
    use serde_json::Value;

    let map = match obj.as_object_mut() {
        Some(m) => m,
        None => {
            // Outer guard already enforced object-ness; defensive only.
            log::warn!(
                "[rtk] ON-sweep: top-level value in {} is not an object; bailing",
                settings_path.display()
            );
            return false;
        }
    };

    // 'hooks' (if present) must be an object.
    if let Some(existing) = map.get("hooks") {
        if !existing.is_object() {
            log::warn!(
                "[rtk] ON-sweep: 'hooks' in {} is {} (expected object); bailing — preserving user data",
                settings_path.display(),
                discriminant_label(existing),
            );
            return false;
        }
    }
    let hooks_obj = map
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .expect("just inserted or pre-checked object");

    // 'PreToolUse' (if present) must be an array.
    if let Some(existing) = hooks_obj.get("PreToolUse") {
        if !existing.is_array() {
            log::warn!(
                "[rtk] ON-sweep: 'hooks.PreToolUse' in {} is {} (expected array); bailing",
                settings_path.display(),
                discriminant_label(existing),
            );
            return false;
        }
    }
    let pretool_arr = hooks_obj
        .entry("PreToolUse".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("just inserted or pre-checked array");

    // Legacy cleanup pre-pass — remove inner hooks whose command contains
    // ANY marker in `RTK_LEGACY_MARKERS`. Runs every ON-sweep so v1
    // entries left on disk by earlier AC builds are cleaned up before the
    // idempotency check (without this, the v2 idempotency check would not
    // find a match, we'd append a v2 entry, and the v1 entry would coexist
    // — Claude Code would dispatch both hooks and the v1 one would still
    // emit the rejected schema).
    let mut legacy_touched_indices: Vec<usize> = Vec::new();
    let mut cleaned_legacy = false;
    for (idx, entry) in pretool_arr.iter_mut().enumerate() {
        let inner = match entry.get_mut("hooks").and_then(|v| v.as_array_mut()) {
            Some(arr) => arr,
            None => continue, // missing or wrong-shape — skip per-entry, preserve user data
        };
        let before = inner.len();
        inner.retain(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|s| !RTK_LEGACY_MARKERS.iter().any(|m| s.contains(m)))
                .unwrap_or(true) // keep entries that don't expose a string command
        });
        if inner.len() != before {
            cleaned_legacy = true;
            legacy_touched_indices.push(idx);
        }
    }
    if cleaned_legacy {
        // Cascade: drop matcher entries we just emptied via legacy cleanup.
        // Mirrors the cascade logic in `remove_rtk_hook`. Only drops entries
        // we touched — user-authored matchers with pre-existing empty hooks
        // arrays are preserved.
        let mut current = 0usize;
        pretool_arr.retain(|entry| {
            let touched = legacy_touched_indices.contains(&current);
            current += 1;
            if !touched {
                return true;
            }
            entry
                .get("hooks")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(true)
        });
    }

    // Idempotency: ANY existing inner hook whose command contains the
    // CURRENT marker means "already applied". This includes user-customized
    // variants — we do NOT overwrite their tweaks. Returns `cleaned_legacy`
    // so the caller writes back if (and only if) the legacy pre-pass
    // actually changed anything.
    for entry in pretool_arr.iter() {
        if let Some(inner) = entry.get("hooks").and_then(|v| v.as_array()) {
            for h in inner {
                if let Some(cmd) = h.get("command").and_then(|c| c.as_str()) {
                    if cmd.contains(RTK_HOOK_MARKER) {
                        return cleaned_legacy;
                    }
                }
            }
        }
    }

    // Find a Bash matcher entry. If multiple exist (rare; user-created),
    // use the FIRST and leave the rest untouched.
    let bash_idx = pretool_arr.iter().position(|entry| {
        entry
            .get("matcher")
            .and_then(|m| m.as_str())
            .map(|s| s == "Bash")
            .unwrap_or(false)
    });

    let our_hook = serde_json::json!({
        "type": "command",
        "command": RTK_REWRITER_COMMAND,
    });

    match bash_idx {
        Some(idx) => {
            // Inner 'hooks' (if present) must be an array.
            if let Some(existing) = pretool_arr[idx].get("hooks") {
                if !existing.is_array() {
                    log::warn!(
                        "[rtk] ON-sweep: 'hooks.PreToolUse[{}].hooks' in {} is {} (expected array); bailing",
                        idx,
                        settings_path.display(),
                        discriminant_label(existing),
                    );
                    return false;
                }
            }
            let entry_obj = pretool_arr[idx]
                .as_object_mut()
                .expect("matcher entry is object (validated by the matcher::as_str above)");
            let inner = entry_obj
                .entry("hooks".to_string())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("just inserted or pre-checked array");
            inner.push(our_hook);
        }
        None => {
            pretool_arr.push(serde_json::json!({
                "matcher": "Bash",
                "hooks": [our_hook],
            }));
        }
    }
    true
}

/// Removes every PreToolUse hook whose `command` contains `RTK_HOOK_MARKER`.
/// Returns `true` iff `obj` was modified (caller should write).
///
/// Wrong-shape branches inside the tree are SKIPPED with a log warn — never
/// destroyed. Other shapes (e.g. a non-Bash matcher entry, an entry with
/// no `hooks` key) are left untouched.
fn remove_rtk_hook(obj: &mut serde_json::Value, settings_path: &Path) -> bool {
    let map = match obj.as_object_mut() {
        Some(m) => m,
        None => return false,
    };

    if !map.contains_key("hooks") {
        return false;
    }
    let hooks_obj = match map.get_mut("hooks").and_then(|v| v.as_object_mut()) {
        Some(o) => o,
        None => {
            log::warn!(
                "[rtk] OFF-sweep: 'hooks' in {} is non-object (preserved as-is)",
                settings_path.display()
            );
            return false;
        }
    };

    if !hooks_obj.contains_key("PreToolUse") {
        return false;
    }
    let pretool_arr = match hooks_obj
        .get_mut("PreToolUse")
        .and_then(|v| v.as_array_mut())
    {
        Some(a) => a,
        None => {
            log::warn!(
                "[rtk] OFF-sweep: 'hooks.PreToolUse' in {} is non-array (preserved as-is)",
                settings_path.display()
            );
            return false;
        }
    };

    // Track WHICH entries we actually emptied via marker removal. Without this,
    // the cascade-retain below would also drop user-authored matcher entries
    // that pre-existed with empty `hooks` arrays — destroying their data even
    // though we never touched them.
    let mut touched_indices: Vec<usize> = Vec::new();
    let mut any_removed = false;
    for (idx, entry) in pretool_arr.iter_mut().enumerate() {
        if let Some(existing) = entry.get("hooks") {
            if !existing.is_array() {
                log::warn!(
                    "[rtk] OFF-sweep: 'hooks.PreToolUse[{}].hooks' in {} is non-array (preserved as-is for this entry)",
                    idx,
                    settings_path.display(),
                );
                continue;
            }
        } else {
            continue;
        }
        let inner = entry
            .get_mut("hooks")
            .and_then(|v| v.as_array_mut())
            .unwrap();
        let before = inner.len();
        inner.retain(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|s| {
                    // Drop entries with the current OR any legacy marker.
                    !s.contains(RTK_HOOK_MARKER)
                        && !RTK_LEGACY_MARKERS.iter().any(|m| s.contains(m))
                })
                .unwrap_or(true) // keep entries that don't expose a string command
        });
        if inner.len() != before {
            any_removed = true;
            touched_indices.push(idx);
        }
    }

    if !any_removed {
        return false;
    }

    // Drop matcher entries we just emptied via marker removal. User-authored
    // entries with pre-existing empty `hooks` arrays are preserved (they are
    // NOT in `touched_indices`).
    let mut current = 0usize;
    pretool_arr.retain(|entry| {
        let touched = touched_indices.contains(&current);
        current += 1;
        if !touched {
            return true; // user's entry — never our concern
        }
        entry
            .get("hooks")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(true) // keep entries with no `hooks` key
    });

    // Cascade: empty PreToolUse → drop key. Empty hooks → drop key.
    if pretool_arr.is_empty() {
        hooks_obj.remove("PreToolUse");
    }
    if hooks_obj.is_empty() {
        map.remove("hooks");
    }

    true
}

/// True iff `path` stat-checks as a real directory — NOT a Unix symlink,
/// NOT a Windows NTFS junction (`FILE_ATTRIBUTE_REPARSE_POINT`), and a
/// directory according to `symlink_metadata`. Consolidates the M7 gate
/// shared by `push_if_new`, the `wg-*` parent re-check, and the parent-dir
/// descend step in `enumerate_managed_agent_dirs` so the filter cannot be
/// regressed by a future caller that forgets one of the three checks.
fn is_real_directory(path: &Path) -> bool {
    let md = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if md.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    md.is_dir()
}

/// Maps a `serde_json::Value` to a short discriminant label for log messages.
fn discriminant_label(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Walks every `<project_root>/<workspace>/` and returns absolute paths to every
/// `_agent_*` matrix and every `__agent_*` replica (inside `wg-*` dirs).
///
/// **`project_paths` semantics.** Each entry may be either (a) a project root
/// that directly contains a Project AC Root, or (b) a parent dir holding many such
/// project roots as immediate children. We probe both shapes — base + non-
/// hidden children — mirroring the existing pattern in
/// `commands/ac_discovery.rs::discover_ac_agents` (~line 596) and
/// `commands/repos.rs::search_repos`. Without descending one level, sweeps
/// silently no-op for users whose `project_paths` lists a parent dir.
///
/// Filters applied (grinch M7):
///   - `symlink_metadata` — Unix symlinks-to-dir are NOT followed.
///   - Windows NTFS junctions (`FILE_ATTRIBUTE_REPARSE_POINT`) are filtered.
///   - Canonical-path dedupe — duplicates resolved (same dir reachable via
///     two `project_paths` entries, or via base + child overlap, lands once).
///
/// Skips silently: missing project paths, non-directory entries, unreadable
/// directories, paths that fail to canonicalize.
///
/// Order is filesystem-iteration order minus duplicates; not sorted. Callers
/// should not rely on stable ordering — sweep is idempotent per dir.
pub fn enumerate_managed_agent_dirs(project_paths: &[String]) -> Vec<std::path::PathBuf> {
    use std::collections::HashSet;
    use std::path::PathBuf;

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();

    let push_if_new = |raw: PathBuf, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>| {
        // M7 gate — reject symlinks/junctions/non-dirs in one place.
        if !is_real_directory(&raw) {
            return;
        }

        // Canonicalize; if it fails (race, permissions), skip.
        let canonical = match std::fs::canonicalize(&raw) {
            Ok(c) => c,
            Err(_) => return,
        };
        if seen.insert(canonical) {
            out.push(raw);
        }
    };

    let scan_workspace =
        |workspace_dir: &std::path::Path, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>| {
            let entries = match std::fs::read_dir(workspace_dir) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!(
                        "[rtk-sweep] Cannot read {} for replica enumeration: {}",
                        workspace_dir.display(),
                        e
                    );
                    return;
                }
            };
            for entry in entries.flatten() {
                let p = entry.path();
                let name = match p.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                if name.starts_with("_agent_") {
                    push_if_new(p, out, seen);
                    continue;
                }

                if name.starts_with("wg-") {
                    // M7 gate — re-check wg-* parent isn't a symlink/junction.
                    if !is_real_directory(&p) {
                        continue;
                    }

                    let wg_entries = match std::fs::read_dir(&p) {
                        Ok(e) => e,
                        Err(e) => {
                            log::warn!(
                                "[rtk-sweep] Cannot read workgroup {} for replica enumeration: {}",
                                p.display(),
                                e
                            );
                            continue;
                        }
                    };
                    for wg_entry in wg_entries.flatten() {
                        let rp = wg_entry.path();
                        let rname = match rp.file_name().and_then(|n| n.to_str()) {
                            Some(n) => n.to_string(),
                            None => continue,
                        };
                        if rname.starts_with("__agent_") {
                            push_if_new(rp, out, seen);
                        }
                    }
                }
            }
        };

    for project in project_paths {
        let base = std::path::Path::new(project);
        if !base.is_dir() {
            continue;
        }

        // Build the candidate list: the base itself, plus its non-hidden
        // immediate children. Each candidate is probed for a Project AC Root. This
        // matches the convention used by `commands/ac_discovery.rs` and
        // `commands/repos.rs`, where `project_paths` may be a parent dir.
        //
        // H1' regression fix: the descend step uses the same M7 gate as
        // `push_if_new` and the `wg-*` parent re-check. Without this, a
        // symlink/junction child (e.g. `parent/Linked -> /elsewhere`) would
        // pass `p.is_dir()` (which follows links) and the sweep would write
        // into `/elsewhere/.ac/_agent_*` outside the declared workspace.
        let mut candidates: Vec<PathBuf> = vec![base.to_path_buf()];
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if !is_real_directory(&p) {
                    continue;
                }
                let name = match p.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                if name.starts_with('.') {
                    continue;
                }
                candidates.push(p);
            }
        }

        for repo_dir in candidates {
            let Some(workspace_dir) = crate::config::workspace::existing_workspace_dir(&repo_dir)
            else {
                continue;
            };
            scan_workspace(&workspace_dir, &mut out, &mut seen);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::PathBuf;

    /// Build a unique tempdir for one test.
    fn tempdir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ac-rtk-{}-{}-{}",
            name,
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("create tempdir");
        path
    }

    fn cleanup(p: &Path) {
        let _ = std::fs::remove_dir_all(p);
    }

    /// Write a `settings.local.json` with the given content. Creates `.claude/`.
    fn seed_settings(dir: &Path, content: &str) {
        let claude_dir = dir.join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("create .claude");
        std::fs::write(claude_dir.join("settings.local.json"), content).expect("write seed");
    }

    fn read_settings(dir: &Path) -> Option<Value> {
        let p = dir.join(".claude").join("settings.local.json");
        if !p.exists() {
            return None;
        }
        let raw = std::fs::read_to_string(&p).expect("read");
        let cleaned = raw.strip_prefix('\u{feff}').unwrap_or(raw.as_str());
        serde_json::from_str(cleaned).ok()
    }

    fn read_settings_raw(dir: &Path) -> Option<String> {
        let p = dir.join(".claude").join("settings.local.json");
        if !p.exists() {
            return None;
        }
        std::fs::read_to_string(&p).ok()
    }

    fn canonical_tree() -> Value {
        json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": RTK_REWRITER_COMMAND,
                    }],
                }],
            },
        })
    }

    #[test]
    fn t01_on_no_file_creates_canonical_tree() {
        let dir = tempdir("t01");
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        assert_eq!(v, canonical_tree());
        cleanup(&dir);
    }

    #[test]
    fn t02_off_no_file_is_noop() {
        let dir = tempdir("t02");
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        assert!(read_settings(&dir).is_none(), "file must NOT be created");
        cleanup(&dir);
    }

    #[test]
    fn t03_on_empty_object_adds_full_tree() {
        let dir = tempdir("t03");
        seed_settings(&dir, "{}");
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        assert_eq!(v, canonical_tree());
        cleanup(&dir);
    }

    #[test]
    fn t04_on_preserves_claude_md_excludes() {
        let dir = tempdir("t04");
        seed_settings(&dir, r#"{"claudeMdExcludes":["x"]}"#);
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        assert_eq!(v["claudeMdExcludes"], json!(["x"]));
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        cleanup(&dir);
    }

    #[test]
    fn t05_on_pushes_bash_alongside_read_matcher() {
        let dir = tempdir("t05");
        seed_settings(
            &dir,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Read","hooks":[{"type":"command","command":"echo read"}]}]}}"#,
        );
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        let arr = v["hooks"]["PreToolUse"].as_array().expect("array");
        assert_eq!(arr.len(), 2, "Read entry preserved + new Bash entry pushed");
        let matchers: Vec<&str> = arr.iter().filter_map(|e| e["matcher"].as_str()).collect();
        assert!(matchers.contains(&"Read"));
        assert!(matchers.contains(&"Bash"));
        cleanup(&dir);
    }

    #[test]
    fn t06_on_appends_to_bash_with_other_hooks() {
        let dir = tempdir("t06");
        seed_settings(
            &dir,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo other"}]}]}}"#,
        );
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        let inner = v["hooks"]["PreToolUse"][0]["hooks"]
            .as_array()
            .expect("inner array");
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[0]["command"], "echo other");
        assert_eq!(inner[1]["command"], RTK_REWRITER_COMMAND);
        cleanup(&dir);
    }

    #[test]
    fn t07_on_with_marker_bearing_is_idempotent_noop() {
        let dir = tempdir("t07");
        let initial = canonical_tree();
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        let raw_before = read_settings_raw(&dir).unwrap();
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        // No write happened — raw bytes preserved.
        let raw_after = read_settings_raw(&dir).unwrap();
        assert_eq!(raw_before, raw_after);
        // Structural equality holds too.
        let v = read_settings(&dir).expect("file present");
        assert_eq!(v, initial);
        cleanup(&dir);
    }

    #[test]
    fn t08_off_removes_ours_keeps_unrelated_bash() {
        let dir = tempdir("t08");
        let initial = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [
                        {"type":"command","command":"echo unrelated"},
                        {"type":"command","command":RTK_REWRITER_COMMAND},
                    ],
                }],
            },
        });
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        let v = read_settings(&dir).expect("file present");
        let inner = v["hooks"]["PreToolUse"][0]["hooks"]
            .as_array()
            .expect("inner array");
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["command"], "echo unrelated");
        cleanup(&dir);
    }

    #[test]
    fn t09_off_only_ours_drops_cascading_keys() {
        let dir = tempdir("t09");
        let initial = json!({
            "claudeMdExcludes": ["x"],
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{"type":"command","command":RTK_REWRITER_COMMAND}],
                }],
            },
        });
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        let v = read_settings(&dir).expect("file present");
        assert_eq!(v, json!({"claudeMdExcludes":["x"]}));
        cleanup(&dir);
    }

    #[test]
    fn t10_on_malformed_preserves_file() {
        let dir = tempdir("t10");
        let original = "{ invalid json no closing brace";
        seed_settings(&dir, original);
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let raw = read_settings_raw(&dir).expect("file present");
        assert_eq!(
            raw, original,
            "malformed file must NOT be overwritten on ON"
        );
        cleanup(&dir);
    }

    #[test]
    fn t11_constant_payload_round_trips() {
        let dir = tempdir("t11");
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        let cmd = v["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command string");
        assert_eq!(cmd, RTK_REWRITER_COMMAND);
        assert!(
            cmd.contains(RTK_HOOK_MARKER),
            "marker must survive round-trip"
        );
        cleanup(&dir);
    }

    #[test]
    fn t12_enumerate_filters_team_repo_nondir() {
        let dir = tempdir("t12");
        let project = dir.join("proj");
        let workspace_dir = project.join(".ac");
        std::fs::create_dir_all(workspace_dir.join("_agent_one")).unwrap();
        std::fs::create_dir_all(workspace_dir.join("wg-1-team").join("__agent_two")).unwrap();
        std::fs::create_dir_all(workspace_dir.join("wg-1-team").join("__agent_three")).unwrap();
        std::fs::create_dir_all(workspace_dir.join("wg-1-team").join("repo-x")).unwrap();
        std::fs::create_dir_all(workspace_dir.join("_team_team")).unwrap();
        std::fs::write(workspace_dir.join("readme.txt"), "hello").unwrap();

        let project_paths = vec![project.to_string_lossy().to_string()];
        let result = enumerate_managed_agent_dirs(&project_paths);
        let names: Vec<String> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(result.len(), 3, "exactly 3 dirs, got {:?}", names);
        assert!(names.contains(&"_agent_one".to_string()));
        assert!(names.contains(&"__agent_two".to_string()));
        assert!(names.contains(&"__agent_three".to_string()));
        cleanup(&dir);
    }

    #[test]
    fn t13_off_malformed_preserves_file() {
        let dir = tempdir("t13");
        let original = "{ invalid json";
        seed_settings(&dir, original);
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        let raw = read_settings_raw(&dir).expect("file present");
        assert_eq!(
            raw, original,
            "malformed file must NOT be overwritten on OFF"
        );
        cleanup(&dir);
    }

    #[test]
    fn t14_constant_matches_source_of_truth() {
        // Reads `repo-AgentsCommander/.claude/settings.json` at test time.
        // From `src-tauri/`, the source `.claude/settings.json` lives one level up.
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let source = std::path::Path::new(&manifest)
            .parent()
            .expect("repo root")
            .join(".claude/settings.json");
        let contents = std::fs::read_to_string(&source).expect("read .claude/settings.json");
        let v: serde_json::Value = serde_json::from_str(&contents).expect("parse");
        let cmd = v["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command path in source-of-truth file");
        assert_eq!(
            cmd,
            RTK_REWRITER_COMMAND,
            "RTK_REWRITER_COMMAND drifted from {}",
            source.display()
        );
    }

    #[test]
    fn t15_on_wrong_shape_hooks_preserves() {
        for body in &[
            r#"{"hooks":null}"#,
            r#"{"hooks":"string"}"#,
            r#"{"hooks":42}"#,
        ] {
            let dir = tempdir("t15");
            seed_settings(&dir, body);
            ensure_rtk_pretool_hook(&dir, true).expect("ok");
            let raw = read_settings_raw(&dir).expect("file present");
            assert_eq!(raw, *body, "wrong-shape hooks must be preserved");
            cleanup(&dir);
        }
    }

    #[test]
    fn t16_on_wrong_shape_pretool_preserves() {
        for body in &[
            r#"{"hooks":{"PreToolUse":"string"}}"#,
            r#"{"hooks":{"PreToolUse":{}}}"#,
        ] {
            let dir = tempdir("t16");
            seed_settings(&dir, body);
            ensure_rtk_pretool_hook(&dir, true).expect("ok");
            let raw = read_settings_raw(&dir).expect("file present");
            assert_eq!(raw, *body, "wrong-shape PreToolUse must be preserved");
            cleanup(&dir);
        }
    }

    #[test]
    fn t17_on_wrong_shape_inner_hooks_preserves() {
        let body = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":"string"}]}}"#;
        let dir = tempdir("t17");
        seed_settings(&dir, body);
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let raw = read_settings_raw(&dir).expect("file present");
        assert_eq!(raw, body, "wrong-shape inner hooks must be preserved");
        cleanup(&dir);
    }

    #[test]
    fn t18_off_wrong_shape_pretool_preserves() {
        let body = r#"{"hooks":{"PreToolUse":"string"}}"#;
        let dir = tempdir("t18");
        seed_settings(&dir, body);
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        let raw = read_settings_raw(&dir).expect("file present");
        assert_eq!(raw, body, "wrong-shape PreToolUse must be preserved on OFF");
        cleanup(&dir);
    }

    #[test]
    fn t19_enumerate_skips_symlinks_and_junctions() {
        let dir = tempdir("t19");
        let project = dir.join("proj");
        let workspace_dir = project.join(".ac");
        let wg = workspace_dir.join("wg-1-team");
        std::fs::create_dir_all(&wg).unwrap();
        let real_target = dir.join("outside-target");
        std::fs::create_dir_all(&real_target).unwrap();

        let link_path = wg.join("__agent_linked");
        let link_created;
        #[cfg(unix)]
        {
            link_created = std::os::unix::fs::symlink(&real_target, &link_path).is_ok();
        }
        #[cfg(windows)]
        {
            // mklink /J creates a junction (no admin required).
            let mut cmd = std::process::Command::new("cmd");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
            let status = cmd
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    link_path.to_str().unwrap(),
                    real_target.to_str().unwrap(),
                ])
                .status();
            link_created = matches!(status, Ok(s) if s.success());
        }

        if !link_created {
            // Test cannot create the link in this environment — skip gracefully.
            cleanup(&dir);
            return;
        }

        // Add a real replica too so we know enumeration ran.
        std::fs::create_dir_all(wg.join("__agent_real")).unwrap();

        let project_paths = vec![project.to_string_lossy().to_string()];
        let result = enumerate_managed_agent_dirs(&project_paths);
        let names: Vec<String> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(
            names.contains(&"__agent_real".to_string()),
            "real replica must be returned: {:?}",
            names
        );
        assert!(
            !names.contains(&"__agent_linked".to_string()),
            "linked replica must be filtered: {:?}",
            names
        );
        cleanup(&dir);
    }

    #[test]
    fn t20_enumerate_dedupes_by_canonical_path() {
        let dir = tempdir("t20");
        let project = dir.join("proj");
        std::fs::create_dir_all(project.join(".ac").join("_agent_one")).unwrap();
        let s = project.to_string_lossy().to_string();
        let project_paths = vec![s.clone(), s];
        let result = enumerate_managed_agent_dirs(&project_paths);
        assert_eq!(result.len(), 1, "duplicates must collapse to 1");
        cleanup(&dir);
    }

    #[test]
    fn t21_marker_idempotency_with_different_body() {
        let dir = tempdir("t21");
        // Hook with the CURRENT marker prefix but a different body —
        // exercises idempotency-by-marker (preserves user customizations of
        // the rewriter body across AC upgrades). Legacy-marker auto-cleanup
        // is exercised separately by t27.
        let older = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"node -e \"'@ac-rtk-marker-v2'; /* USER-CUSTOMIZED REWRITER BODY */\""}]}]}}"#;
        seed_settings(&dir, older);
        // ON: idempotent (no-op).
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let raw_after_on = read_settings_raw(&dir).expect("file present");
        assert_eq!(
            raw_after_on, older,
            "ON with current-marker-bearing entry must be a structural no-op"
        );
        // OFF: removes the marker-bearing entry → cascade to {}.
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        let v_after_off = read_settings(&dir).expect("file present after OFF");
        assert_eq!(
            v_after_off,
            json!({}),
            "OFF must filter by marker substring"
        );
        cleanup(&dir);
    }

    #[test]
    fn t22_bom_handling_on() {
        let dir = tempdir("t22-on");
        let with_bom = "\u{feff}{\"claudeMdExcludes\":[]}";
        seed_settings(&dir, with_bom);
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        assert_eq!(v["claudeMdExcludes"], json!([]));
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        let raw = read_settings_raw(&dir).unwrap();
        assert!(
            !raw.starts_with('\u{feff}'),
            "BOM must be stripped on write"
        );
        cleanup(&dir);
    }

    #[test]
    fn t23_enumerate_descends_one_level_for_parent_project_paths() {
        // project_paths may be either a project root (containing .ac/)
        // or a parent dir holding many such roots. Mirrors the convention in
        // commands/ac_discovery.rs::discover_ac_agents — without descending
        // one level, sweeps silently no-op for the parent-dir shape.
        let dir = tempdir("t23");
        let parent = dir.join("parent");
        let proj_a = parent.join("AppA");
        let proj_b = parent.join("AppB");
        std::fs::create_dir_all(proj_a.join(".ac").join("_agent_alpha")).unwrap();
        std::fs::create_dir_all(proj_b.join(".ac").join("wg-1-team").join("__agent_beta")).unwrap();
        // A hidden child should be skipped (matching ac_discovery convention).
        std::fs::create_dir_all(parent.join(".hidden")).unwrap();

        let project_paths = vec![parent.to_string_lossy().to_string()];
        let result = enumerate_managed_agent_dirs(&project_paths);
        let names: Vec<String> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(
            names.contains(&"_agent_alpha".to_string()),
            "matrix in child project must be returned: {:?}",
            names
        );
        assert!(
            names.contains(&"__agent_beta".to_string()),
            "replica in child project must be returned: {:?}",
            names
        );
        cleanup(&dir);
    }

    #[test]
    fn t24_off_preserves_user_authored_empty_matchers() {
        // A user may pre-author a matcher entry with an empty hooks array
        // (legal-ish JSON; some hand-craft these). The OFF cascade-retain
        // must NOT confuse "we just emptied this" with "this was already
        // empty" and must preserve user-authored entries we never touched.
        let dir = tempdir("t24");
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Read", "hooks": []},
                    {"matcher": "Bash", "hooks": [{"type":"command","command":RTK_REWRITER_COMMAND}]},
                ],
            },
        });
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        let v = read_settings(&dir).expect("file present");
        let arr = v["hooks"]["PreToolUse"].as_array().expect("array");
        // The Bash entry was emptied by our removal → should be dropped.
        // The Read entry was never touched → should be preserved.
        assert_eq!(
            arr.len(),
            1,
            "exactly one entry remains (the user's untouched Read entry); got {:?}",
            arr
        );
        assert_eq!(arr[0]["matcher"], "Read");
        let inner = arr[0]["hooks"].as_array().expect("inner array");
        assert!(inner.is_empty(), "user's empty hooks array preserved");
        cleanup(&dir);
    }

    #[test]
    fn t25_enumerate_descend_skips_symlinked_child() {
        // project_paths = parent dir; one of parent's non-hidden children is a
        // symlink/junction pointing OUTSIDE the parent tree to a dir that has
        // its own .ac/. The descend must NOT cross that boundary.
        // (Grinch H1' regression test — without the M7 gate at the descend
        // step, the sweep escapes the declared workspace.)
        let dir = tempdir("t25");
        let parent = dir.join("parent");
        let real_proj = parent.join("AppA");
        std::fs::create_dir_all(real_proj.join(".ac").join("_agent_real")).unwrap();

        // Create the would-be-leaked target outside `parent`, with its own .ac.
        let outside = dir.join("outside");
        std::fs::create_dir_all(outside.join(".ac").join("_agent_outside")).unwrap();

        // Symlink/junction parent/Linked -> outside.
        let link = parent.join("Linked");
        let link_created;
        #[cfg(unix)]
        {
            link_created = std::os::unix::fs::symlink(&outside, &link).is_ok();
        }
        #[cfg(windows)]
        {
            let mut cmd = std::process::Command::new("cmd");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
            let status = cmd
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    link.to_str().unwrap(),
                    outside.to_str().unwrap(),
                ])
                .status();
            link_created = matches!(status, Ok(s) if s.success());
        }
        if !link_created {
            cleanup(&dir);
            return; // skip when env can't create links
        }

        let project_paths = vec![parent.to_string_lossy().to_string()];
        let result = enumerate_managed_agent_dirs(&project_paths);
        let names: Vec<String> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(
            names.contains(&"_agent_real".to_string()),
            "real project's matrix must be enumerated: {:?}",
            names
        );
        assert!(
            !names.contains(&"_agent_outside".to_string()),
            "symlinked-target matrix must NOT be enumerated — sweep escape: {:?}",
            names
        );
        cleanup(&dir);
    }

    #[test]
    fn t26_hook_output_matches_claude_code_v2_schema() {
        // Regression sentinel for the v1→v2 schema bump: actually run the
        // injected JS via node and assert the output shape is the v2 form
        // Claude Code current accepts (`hookSpecificOutput.updatedInput`).
        // A regression to the v1 `decision:'modify'` shape would slip past
        // every other test (which compares JSON structure, not runtime
        // behavior of the JS body).
        use std::io::Write;
        use std::process::{Command, Stdio};

        // Skip gracefully if node isn't in PATH (CI without node, etc.).
        let mut node_check_cmd = Command::new("node");
        crate::pty::credentials::scrub_credentials_from_std_command(&mut node_check_cmd);
        let node_check = node_check_cmd.arg("--version").output();
        if !matches!(&node_check, Ok(o) if o.status.success()) {
            return;
        }

        let input = serde_json::json!({"tool_input": {"command": "git status"}});

        // Extract the JS body from RTK_REWRITER_COMMAND. The const has shape
        // `node -e "<JS>"` so we strip the leading `node -e "` and trailing `"`.
        let js_body = RTK_REWRITER_COMMAND
            .strip_prefix("node -e \"")
            .and_then(|s| s.strip_suffix('"'))
            .expect("RTK_REWRITER_COMMAND must have shape `node -e \"...\"`");

        let mut node_cmd = Command::new("node");
        crate::pty::credentials::scrub_credentials_from_std_command(&mut node_cmd);
        let mut child = node_cmd
            .arg("-e")
            .arg(js_body)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("node spawn");
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .expect("write stdin");
        let output = child.wait_with_output().expect("hook ran");
        assert!(
            output.status.success(),
            "hook exit non-zero: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(stdout.trim()).expect("hook stdout must be valid JSON");
        let updated_cmd = parsed["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .expect("v2 shape: hookSpecificOutput.updatedInput.command must be a string");
        assert_eq!(
            updated_cmd, "rtk git status",
            "rewriter must prefix command with 'rtk '"
        );
        let event = parsed["hookSpecificOutput"]["hookEventName"]
            .as_str()
            .expect("hookEventName must be string");
        assert_eq!(event, "PreToolUse");
        // Negative check: v1 shape MUST NOT appear.
        assert!(
            parsed.get("decision").is_none(),
            "v1 shape leaked: top-level `decision` field present"
        );
        assert!(
            parsed.get("tool_input").is_none(),
            "v1 shape leaked: top-level `tool_input` field present"
        );
    }

    #[test]
    fn t27_legacy_marker_cleanup_on_on_sweep() {
        // Auto-migration: a replica with a v1-marker entry (broken pre-fix
        // shape) must be cleaned by ON-sweep before the v2 entry is
        // inserted. Without this the v2 idempotency check would not match,
        // a v2 entry would be appended, and Claude Code would dispatch
        // BOTH hooks — the v1 one would still emit the rejected schema.
        let dir = tempdir("t27");
        let initial = json!({
            "claudeMdExcludes": ["x"],
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "node -e \"'@ac-rtk-marker-v1'; /* old broken v1 body */\"",
                    }],
                }],
            },
        });
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        // Other keys preserved.
        assert_eq!(v["claudeMdExcludes"], json!(["x"]));
        // Bash matcher present, exactly one inner hook (the new v2), legacy gone.
        let inner = v["hooks"]["PreToolUse"][0]["hooks"]
            .as_array()
            .expect("inner");
        assert_eq!(
            inner.len(),
            1,
            "v1 entry should be cleaned, only v2 remains: {:?}",
            inner
        );
        let cmd = inner[0]["command"].as_str().expect("string");
        assert!(cmd.contains(RTK_HOOK_MARKER), "v2 marker present: {}", cmd);
        assert!(
            !cmd.contains("@ac-rtk-marker-v1"),
            "v1 marker gone: {}",
            cmd
        );
        cleanup(&dir);
    }

    #[test]
    fn t28_off_sweep_cleans_legacy_markers() {
        // OFF-sweep removes both current AND legacy markers, leaving
        // unrelated user hooks untouched.
        let dir = tempdir("t28");
        let initial = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [
                        {"type": "command", "command": "node -e \"'@ac-rtk-marker-v1'; legacy\""},
                        {"type": "command", "command": RTK_REWRITER_COMMAND},
                        {"type": "command", "command": "echo unrelated"},
                    ],
                }],
            },
        });
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        let v = read_settings(&dir).expect("file present");
        let inner = v["hooks"]["PreToolUse"][0]["hooks"]
            .as_array()
            .expect("inner");
        assert_eq!(
            inner.len(),
            1,
            "v1 + v2 both cleaned, only unrelated remains: {:?}",
            inner
        );
        assert_eq!(inner[0]["command"], "echo unrelated");
        cleanup(&dir);
    }

    #[test]
    fn rtk_hook_marker_literal_is_locked() {
        // The marker substring is the public contract for cross-version
        // ON-idempotency and OFF-cleanup. A future "let me clean up this
        // magic string" refactor that changes the literal silently breaks
        // every replica injected by an older AC build. Bumped to v2 when
        // the hook output schema changed; v1 retained in
        // `RTK_LEGACY_MARKERS` for cleanup.
        assert_eq!(RTK_HOOK_MARKER, "@ac-rtk-marker-v2");
        assert_eq!(RTK_LEGACY_MARKERS, &["@ac-rtk-marker-v1"]);

        // Forward-compat guard: no current marker can be a substring of any
        // legacy marker, and no legacy marker can be a substring of the
        // current one. Today this holds (`v2` vs `v1`), but if a future
        // bumper goes from v1 to v10 keeping v1 in RTK_LEGACY_MARKERS, the
        // legacy filter `s.contains("@ac-rtk-marker-v1")` would match the
        // current `@ac-rtk-marker-v10` substring and corrupt every sweep.
        // This assertion forces the next bumper to confront the issue at
        // edit time rather than at production-runtime.
        for legacy in RTK_LEGACY_MARKERS {
            assert!(
                !RTK_HOOK_MARKER.contains(legacy),
                "RTK_HOOK_MARKER {:?} must not contain legacy marker {:?}",
                RTK_HOOK_MARKER,
                legacy
            );
            assert!(
                !legacy.contains(RTK_HOOK_MARKER),
                "legacy marker {:?} must not contain RTK_HOOK_MARKER {:?}",
                legacy,
                RTK_HOOK_MARKER
            );
        }
    }

    #[test]
    fn t29_v1_and_v2_coexist_legacy_cleaned_v2_preserved() {
        // Regression for the exact race the v2 patch was written to handle.
        // A user who manually migrated may have BOTH a v1 entry (broken
        // shape, dispatched by Claude Code with rejected output) and a v2
        // entry on disk simultaneously. ON-sweep must clean v1, preserve
        // v2 unchanged, and write back (because legacy cleanup mutated).
        let dir = tempdir("t29");
        let initial = json!({
            "hooks": {"PreToolUse": [{
                "matcher": "Bash",
                "hooks": [
                    {"type": "command", "command": "node -e \"'@ac-rtk-marker-v1'; old\""},
                    {"type": "command", "command": RTK_REWRITER_COMMAND},
                ],
            }]},
        });
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        let raw_before = read_settings_raw(&dir).unwrap();
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let raw_after = read_settings_raw(&dir).unwrap();
        assert_ne!(
            raw_before, raw_after,
            "legacy cleanup must trigger a write back to disk"
        );
        let v = read_settings(&dir).expect("file present");
        let inner = v["hooks"]["PreToolUse"][0]["hooks"]
            .as_array()
            .expect("inner");
        assert_eq!(inner.len(), 1, "exactly the v2 entry remains: {:?}", inner);
        assert_eq!(inner[0]["command"], RTK_REWRITER_COMMAND);
        cleanup(&dir);
    }

    #[test]
    fn t30_v1_alongside_user_hook_preserves_user_appends_v2() {
        // Cascade-drop must NOT remove a Bash matcher entry just because
        // we touched it during legacy cleanup — only when its inner hooks
        // are now empty. If the user has an unrelated hook in the same
        // matcher, the cascade keeps the matcher and ON-sweep then appends
        // a v2 hook next to the user's existing one.
        let dir = tempdir("t30");
        let initial = json!({
            "hooks": {"PreToolUse": [{
                "matcher": "Bash",
                "hooks": [
                    {"type": "command", "command": "echo other"},
                    {"type": "command", "command": "node -e \"'@ac-rtk-marker-v1'; old\""},
                ],
            }]},
        });
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        let arr = v["hooks"]["PreToolUse"].as_array().expect("array");
        assert_eq!(arr.len(), 1, "single Bash matcher preserved: {:?}", arr);
        let inner = arr[0]["hooks"].as_array().expect("inner");
        assert_eq!(
            inner.len(),
            2,
            "user hook + new v2 (v1 cleaned): {:?}",
            inner
        );
        assert_eq!(inner[0]["command"], "echo other", "user hook still first");
        let v2 = inner[1]["command"].as_str().expect("string");
        assert!(
            v2.contains(RTK_HOOK_MARKER) && !v2.contains("@ac-rtk-marker-v1"),
            "second entry is fresh v2: {}",
            v2
        );
        cleanup(&dir);
    }

    #[test]
    fn t31_legacy_cleanup_skips_wrong_shape_entry_per_entry() {
        // The legacy cleanup pre-pass uses per-entry `continue` on wrong-
        // shape inner hooks (the `match get_mut.and_then.as_array_mut`
        // pattern). A regression that swapped `continue` for `return false`
        // would bail the whole sweep on encountering one user-authored
        // wrong-shape entry — even though the next Bash entry has a real
        // v1 marker that must still be cleaned.
        let dir = tempdir("t31");
        let initial = json!({
            "hooks": {"PreToolUse": [
                // First entry: wrong-shape inner.hooks (string, not array).
                // Per-entry skip should preserve it untouched.
                {"matcher": "Read", "hooks": "broken"},
                // Second entry: real v1 marker that legacy cleanup must
                // still process.
                {"matcher": "Bash", "hooks": [
                    {"type": "command", "command": "node -e \"'@ac-rtk-marker-v1'; old\""},
                ]},
            ]},
        });
        seed_settings(&dir, &serde_json::to_string(&initial).unwrap());
        ensure_rtk_pretool_hook(&dir, true).expect("ok");
        let v = read_settings(&dir).expect("file present");
        let arr = v["hooks"]["PreToolUse"].as_array().expect("array");
        // First entry preserved as-is (still has wrong-shape inner.hooks).
        let read_entry = arr.iter().find(|e| e["matcher"] == "Read").expect("Read");
        assert_eq!(
            read_entry["hooks"], "broken",
            "wrong-shape Read entry preserved: {:?}",
            read_entry
        );
        // Second entry (Bash) had its v1 entry cleaned + a v2 appended.
        let bash_entry = arr.iter().find(|e| e["matcher"] == "Bash").expect("Bash");
        let inner = bash_entry["hooks"].as_array().expect("inner");
        assert_eq!(inner.len(), 1, "v1 cleaned + v2 appended: {:?}", inner);
        let cmd = inner[0]["command"].as_str().expect("string");
        assert!(
            cmd.contains(RTK_HOOK_MARKER) && !cmd.contains("@ac-rtk-marker-v1"),
            "Bash entry holds the fresh v2: {}",
            cmd
        );
        cleanup(&dir);
    }

    #[test]
    fn t22_bom_handling_off() {
        let dir = tempdir("t22-off");
        let canonical_minified = serde_json::to_string(&canonical_tree()).unwrap();
        let with_bom = format!("\u{feff}{}", canonical_minified);
        seed_settings(&dir, &with_bom);
        ensure_rtk_pretool_hook(&dir, false).expect("ok");
        let v = read_settings(&dir).expect("file present");
        assert_eq!(v, json!({}), "OFF removes our hook even with BOM");
        let raw = read_settings_raw(&dir).unwrap();
        assert!(
            !raw.starts_with('\u{feff}'),
            "BOM must be stripped on write"
        );
        cleanup(&dir);
    }
}
