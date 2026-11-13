//! #2774 Hyprland key binding: one AC-managed block per instance in `bindings.lua`, and
//! detection of a binding the user added by hand. Leaf module: env is passed in.

use std::path::Path;

use crate::filesystem::hyprland_bindings::{
    omarchy_bindings_path, read_bindings, replace_bindings_if_unchanged, BindingsWriteError,
};

/// `modmask` of CTRL in `hyprctl binds -j`.
const CTRL_MODMASK: u64 = 4;
const DAMAGED: &str =
    "The AgentsCommander lines in bindings.lua are damaged; delete them and save Settings again.";
const NOT_OMARCHY: &str = "bindings.lua is not the Omarchy layout \
    (a regular ~/.config/hypr/bindings.lua loaded by hyprland.lua)";
const NOT_LOADED_UNDONE: &str = "Hyprland did not load the AgentsCommander binding from \
    bindings.lua; the change was undone.";

pub(crate) struct BindIds<'a> {
    pub(crate) token: &'a str,
    pub(crate) global: &'a str,
    pub(crate) description: &'a str,
}

pub(crate) fn lua_line(key: char, ids: &BindIds) -> String {
    let key = key.to_ascii_uppercase();
    let (global, description) = (ids.global, ids.description);
    format!(
        "hl.bind(\"CTRL + {key}\", hl.dsp.global(\"{global}\"), \
         {{ description = \"{description}\" }})"
    )
}

pub(crate) fn conf_line(key: char, ids: &BindIds) -> String {
    let key = key.to_ascii_uppercase();
    format!(
        "bindd = CTRL, {key}, {}, global, {}",
        ids.description, ids.global
    )
}

fn begin_marker(token: &str) -> String {
    format!("-- >>> AgentsCommander screenshot shortcut {token} (managed by AgentsCommander) >>>")
}

fn end_marker(token: &str) -> String {
    format!("-- <<< AgentsCommander screenshot shortcut {token} <<<")
}

fn block(key: char, ids: &BindIds) -> String {
    let (begin, end) = (begin_marker(ids.token), end_marker(ids.token));
    format!("{begin}\n{}\n{end}\n", lua_line(key, ids))
}

/// Byte ranges (line start, end after its `\n`) of the lines equal to `marker`.
fn marker_lines(content: &str, marker: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut start = 0;
    for line in content.split_inclusive('\n') {
        let end = start + line.len();
        let text = line.strip_suffix('\n').unwrap_or(line);
        if text.strip_suffix('\r').unwrap_or(text) == marker {
            found.push((start, end));
        }
        start = end;
    }
    found
}

/// Put `block` in place of this token's block (append if absent), or remove it (`None`).
fn splice(content: &str, token: &str, block: Option<&str>) -> Result<String, String> {
    let begins = marker_lines(content, &begin_marker(token));
    let ends = marker_lines(content, &end_marker(token));
    let range = match (begins.as_slice(), ends.as_slice()) {
        ([], []) => None,
        ([begin], [end]) if end.0 >= begin.1 => Some((begin.0, end.1)),
        _ => return Err(DAMAGED.to_string()),
    };
    let mut out = String::with_capacity(content.len() + block.map_or(0, str::len) + 2);
    match (range, block) {
        (Some((start, end)), Some(b)) => {
            out.push_str(&content[..start]);
            out.push_str(b);
            out.push_str(&content[end..]);
        }
        (Some((start, end)), None) => {
            let before = &content[..start];
            out.push_str(
                before
                    .strip_suffix('\n')
                    .filter(|b| b.ends_with('\n'))
                    .unwrap_or(before),
            );
            out.push_str(&content[end..]);
        }
        (None, Some(b)) => {
            out.push_str(content);
            if !content.is_empty() {
                if !content.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(b);
        }
        (None, None) => out.push_str(content),
    }
    Ok(out)
}

pub(crate) trait Hyprctl: Sync {
    fn run(
        &self,
        args: &'static [&'static str],
    ) -> impl std::future::Future<Output = Result<String, String>> + Send;
}

pub(crate) struct SystemHyprctl;

impl Hyprctl for SystemHyprctl {
    async fn run(&self, args: &'static [&'static str]) -> Result<String, String> {
        crate::process::hyprctl(args).await
    }
}

/// `hyprctl configerrors -j`: a JSON array of strings; blank items are no errors.
fn parse_config_errors(stdout: &str) -> Result<Vec<String>, String> {
    let items: Vec<String> = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    Ok(items.into_iter().filter(|e| !e.trim().is_empty()).collect())
}

/// `hyprctl binds -j`: Lua binds show `dispatcher: "__lua"`, so the description is the
/// only reliable owner mark.
fn parse_bound(stdout: &str, key: char, description: &str) -> Result<bool, String> {
    let binds: Vec<serde_json::Value> = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let key = key.to_string();
    Ok(binds.iter().any(|b| {
        b["description"].as_str() == Some(description)
            && b["modmask"].as_u64() == Some(CTRL_MODMASK)
            && b["key"]
                .as_str()
                .is_some_and(|k| k.eq_ignore_ascii_case(&key))
    }))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BindOutcome {
    NotHyprland,
    Applied,
    UserBound,
    NotLoaded,
    Manual { reason: Option<String> },
    Failed(String),
}

fn write_error_text(error: BindingsWriteError) -> String {
    match error {
        BindingsWriteError::Changed => "bindings.lua was changed meanwhile".to_string(),
        BindingsWriteError::Io(m) => m,
    }
}

async fn config_errors<C: Hyprctl>(ctl: &C) -> Result<Vec<String>, String> {
    parse_config_errors(&ctl.run(&["configerrors", "-j"]).await?)
}

async fn reload<C: Hyprctl>(ctl: &C) {
    match ctl.run(&["reload"]).await {
        Ok(out) if out.trim() == "ok" => {}
        other => log::warn!("[screenshot] hyprctl reload: {other:?}"),
    }
}

/// Write `next` over `original`, reload, and undo it if Hyprland reports new errors.
async fn write_verified<C: Hyprctl>(
    ctl: &C,
    path: &Path,
    original: &str,
    next: &str,
) -> Result<(), String> {
    let before = config_errors(ctl)
        .await
        .map_err(|e| format!("could not run hyprctl: {e}"))?;
    replace_bindings_if_unchanged(path, original, next).map_err(|e| match e {
        BindingsWriteError::Changed => {
            "bindings.lua changed while AgentsCommander was updating it; nothing was written."
                .to_string()
        }
        BindingsWriteError::Io(m) => format!("could not write bindings.lua: {m}"),
    })?;
    reload(ctl).await;
    let first = match config_errors(ctl).await {
        Ok(after) => match after.into_iter().find(|e| !before.contains(e)) {
            None => return Ok(()),
            Some(first) => first,
        },
        Err(e) => e,
    };
    let restored = replace_bindings_if_unchanged(path, next, original);
    reload(ctl).await;
    let prefix = format!("Hyprland rejected the AgentsCommander binding ({first})");
    Err(match restored {
        Ok(()) => format!("{prefix}; bindings.lua was restored."),
        Err(e) => format!(
            "{prefix}; restoring bindings.lua failed: {}",
            write_error_text(e)
        ),
    })
}

pub(crate) async fn apply<C: Hyprctl>(
    ctl: &C,
    hyprland: bool,
    config_home: Option<&Path>,
    enabled: bool,
    key: char,
    ids: &BindIds<'_>,
) -> BindOutcome {
    if !hyprland {
        return BindOutcome::NotHyprland;
    }
    let path = config_home.and_then(omarchy_bindings_path);
    let reason = match config_home {
        None => "HOME is not set",
        Some(_) => NOT_OMARCHY,
    };
    let mut written = None;
    if let Some(path) = &path {
        let original = match read_bindings(path) {
            Ok(text) => text,
            Err(m) => return BindOutcome::Failed(m),
        };
        let wanted = enabled.then(|| block(key, ids));
        let next = match splice(&original, ids.token, wanted.as_deref()) {
            Ok(next) => next,
            Err(m) => return BindOutcome::Failed(m),
        };
        if next != original {
            if let Err(m) = write_verified(ctl, path, &original, &next).await {
                return BindOutcome::Failed(m);
            }
            if enabled {
                written = Some((original, next));
            }
        }
    }
    let bound = match ctl.run(&["binds", "-j"]).await {
        Ok(out) => parse_bound(&out, key, ids.description),
        Err(e) => Err(e),
    };
    let bound = match bound {
        Ok(bound) => bound,
        Err(e) => {
            let m = format!("could not read the Hyprland key bindings: {e}");
            return BindOutcome::Failed(m);
        }
    };
    match (bound, enabled, &path, written) {
        (true, true, Some(_), _) => BindOutcome::Applied,
        (true, ..) => BindOutcome::UserBound,
        (false, _, Some(path), Some((original, next))) => {
            if let Err(e) = replace_bindings_if_unchanged(path, &next, &original) {
                log::warn!("[screenshot] undo bindings.lua: {}", write_error_text(e));
            }
            reload(ctl).await;
            BindOutcome::Failed(NOT_LOADED_UNDONE.to_string())
        }
        (false, true, Some(_), None) => BindOutcome::NotLoaded,
        (false, true, None, _) => BindOutcome::Manual {
            reason: Some(reason.to_string()),
        },
        (false, false, ..) => BindOutcome::Manual { reason: None },
    }
}

/// The hotkey status for `outcome`: what works, and what to paste otherwise.
pub(crate) fn status_result(outcome: &BindOutcome, key: char, ids: &BindIds) -> Result<(), String> {
    let lua = lua_line(key, ids);
    match outcome {
        BindOutcome::NotHyprland | BindOutcome::Applied | BindOutcome::UserBound => Ok(()),
        BindOutcome::NotLoaded => Err("bindings.lua has the AgentsCommander binding, but \
            Hyprland has not loaded it. Fix the errors that `hyprctl configerrors` lists, then \
            save Settings again."
            .to_string()),
        BindOutcome::Manual { reason: None } => Err(format!(
            "Hyprland needs a key binding for it. Add this line to \
             ~/.config/hypr/bindings.lua: {lua} (in hyprland.conf: {}). Or turn on \"Add the \
             screenshot shortcut to Hyprland for me\" in Settings > Hotkeys.",
            conf_line(key, ids)
        )),
        BindOutcome::Manual { reason: Some(r) } => Err(format!(
            "AgentsCommander cannot add the key binding for you: {r}. Add this line to your \
             Hyprland config: {lua} (in hyprland.conf: {}).",
            conf_line(key, ids)
        )),
        BindOutcome::Failed(m) => Err(format!(
            "{m} Add this line to your Hyprland config instead: {lua}"
        )),
    }
}

#[cfg(test)]
mod tests;
