//! #4780: the AgentsCommander context of a temp room (limited, or lifted once Unleashed).
//! Layering: uses only `crate::config` leaves and `crate::filesystem::session_context`.

use crate::config::instance_artifacts::CONTEXT_CACHE_DIR_NAME;
use std::path::Path;

const HEADER: &str = concat!(
    "# AgentsCommander Context — Temporary Room\n",
    "\n",
    "You are a coding agent in an AgentsCommander Temporary Room (`{ROOM}`). It has no team, no\n",
    "project and no repos. Your working folder is:\n",
    "\n",
    "`{ROOT}`\n",
);

const LIMIT: &str = concat!(
    "\n",
    "## GOLDEN RULE — Temporary Room Access Limit\n",
    "\n",
    "**ABSOLUTE AND NON-NEGOTIABLE:** read, list, search and write only inside your working\n",
    "folder and its descendants (this includes its `messaging/` folder).\n",
    "\n",
    "Everything else is OFF-LIMITS, in particular the parent folder `{CONFIG}` and all its\n",
    "other contents:\n",
    "- `master-token.txt`, `settings.json` and every `settings*.json`, `sessions.json`,\n",
    "  `unleashed-replicas.json` and the context cache: never read, copy, print or use them;\n",
    "- other Temporary Rooms (`temp-room-tr*`) and the Root Agent folder (`ac-root-agent`);\n",
    "- the user home, other projects, rooms and agents.\n",
    "\n",
    "Use no token except `AGENTSCOMMANDER_TOKEN`. If a task needs anything outside your folder,\n",
    "stop and ask the user (the user can Unleash this agent).\n",
    "\n",
    "Incoming-message exception: you MAY READ exactly one message file outside your folder\n",
    "when AgentsCommander gives the canonical `[Message from <peer>] Process this inter-agent\n",
    "message: <path>` notification for it, or the user names that exact file to you. Such a\n",
    "file lies in a room's `messaging` folder or in `ac-root-agent/messaging/`. This exception\n",
    "wins over the list above for that one file only. It is read-only: do not list or explore\n",
    "that folder, read any other file there, or create, change or delete anything there.\n",
    "\n",
    "CLI exception: commands run through `AGENTSCOMMANDER_BINARY_PATH` may act outside your\n",
    "folder; AgentsCommander governs their effects.\n",
);

const LIFTED: &str = concat!(
    "\n",
    "## Unleashed\n",
    "\n",
    "The user Unleashed this agent: the Temporary Room access limit is lifted. The CLI,\n",
    "messaging and integrity rules below still apply.\n",
);

const COMMON: &str = concat!(
    "\n",
    "## AgentsCommander CLI\n",
    "\n",
    "Credentials: `AGENTSCOMMANDER_TOKEN`, `AGENTSCOMMANDER_ROOT`, `AGENTSCOMMANDER_BINARY`,\n",
    "`AGENTSCOMMANDER_BINARY_PATH`, `AGENTSCOMMANDER_LOCAL_DIR`. Invoke only\n",
    "`AGENTSCOMMANDER_BINARY_PATH`; use `--help` for anything not documented here. CLI task\n",
    "commands do not apply in Temporary Rooms.\n",
    "\n",
    "## Inter-Agent Messaging\n",
    "\n",
    "Your address is `{ADDR}`. You may message only the Root Agent\n",
    "(`agentscommander://root-agent`) and verified room orchestrators, and only they can\n",
    "message you.\n",
    "\n",
    "Incoming: for \"[Message from <peer>] Process this inter-agent message: <path>\", read that\n",
    "exact file (the incoming-message exception covers it) and follow it within these rules.\n",
    "When done or blocked, reply to the sender.\n",
    "\n",
    "To send:\n",
    "1. `\"$AGENTSCOMMANDER_BINARY_PATH\" list-peers-lean --token \"$AGENTSCOMMANDER_TOKEN\"\n",
    "   --root \"$AGENTSCOMMANDER_ROOT\"`; use an exact `name` as `--to`. ",
    "Empty `[]`: stop, report.\n",
    "2. Write a new file in `{ROOT}/messaging/` named\n",
    "   `YYYYMMDD-HHMMSS-{ROOM}-to-<peer>-<slug>.md` (UTC, lowercase; `<peer>` is `root` or\n",
    "   `roomN-<agent>`; kebab-case slug of at most 50 chars without the word `to`).\n",
    "3. `\"$AGENTSCOMMANDER_BINARY_PATH\" send --token \"$AGENTSCOMMANDER_TOKEN\" --root\n",
    "   \"$AGENTSCOMMANDER_ROOT\" --to \"<name>\" --send <filename> --mode wake` ",
    "(file name only).\n",
    "   A send counts only with a `Queued: <message-id>` line. Do not use `--get-output`.\n",
    "\n",
    "## Integrity Rules\n",
    "\n",
    "In `{ROOT}/messaging/`, write only message files; never modify or delete one once written.\n",
    "\n",
    "## Host Platform Rules\n",
    "\n",
    "{PLATFORM}\n",
);

/// The exact temp-room context text; `lifted` selects the Unleashed form.
pub(crate) fn render_temp_room_context(
    root: &str,
    room_name: &str,
    config_dir: &str,
    lifted: bool,
    platform_rules: &str,
) -> String {
    let root = root.strip_prefix(r"\\?\").unwrap_or(root);
    let room_id = room_name
        .strip_prefix(crate::config::temp_rooms::TEMP_ROOM_PREFIX)
        .map_or_else(|| room_name.to_string(), |digits| format!("tr{digits}"));
    let addr = format!("agentscommander://temp-room/{room_id}");
    let mut text = String::from(HEADER);
    text.push_str(if lifted { LIFTED } else { LIMIT });
    text.push_str(COMMON);
    // PLATFORM last so caller text is never rescanned for placeholders.
    text.replace("{ROOT}", root)
        .replace("{ROOM}", room_name)
        .replace("{ADDR}", &addr)
        .replace("{CONFIG}", config_dir)
        .replace("{PLATFORM}", platform_rules)
}

/// Renders the context for temp-room `cwd` into the context cache; returns the cache path.
pub(crate) fn build_temp_room_context(cwd: &str, platform_rules: &str) -> Result<String, String> {
    let config_dir = crate::config::config_dir()
        .ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let room_name = Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Temp room path has no folder name: {cwd}"))?;
    let lifted = crate::config::unleashed_replicas::is_unleashed(cwd);
    let content = render_temp_room_context(
        cwd,
        room_name,
        &config_dir.to_string_lossy(),
        lifted,
        platform_rules,
    );
    let context_dir = config_dir.join(CONTEXT_CACHE_DIR_NAME);
    crate::filesystem::session_context::ensure_context_directory(&context_dir).map_err(|e| {
        format!(
            "Failed to create context dir {}: {e}",
            context_dir.display()
        )
    })?;
    let path = context_dir.join(format!("ac-context-{room_name}.md"));
    crate::filesystem::session_context::write_context_cache(&path, content)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests;
