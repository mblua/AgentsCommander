use super::{
    display_path, render_project_shared_entry, render_room_shared_entry, replica_shared_roots,
    DefaultContextDynamicValues, DIRECT_MATRIX_GIT_SCOPE, ROOT_AUTHORITY_SECTION, ROOT_GIT_SCOPE,
    ROOT_PROJECT_SCOPE_ENTRY, WORKGROUP_GIT_SCOPE,
};
#[cfg(test)]
use super::{render_default_agent_context, render_root_runtime_prologue_inner};
#[cfg(test)]
use std::path::Path;

pub(crate) fn default_context_dynamic_values(
    agent_root: &str,
    matrix_root: Option<&str>,
    _skills_section: &str,
    is_root_agent: bool,
) -> DefaultContextDynamicValues {
    // L2 (grinch): a path-based root agent is never a `__agent_*` replica, so it
    // has no origin matrix. Lock the single item-"3." invariant the renderer
    // relies on (Note #6) so a future caller cannot emit two "3." entries by
    // passing both a matrix and the root flag.
    debug_assert!(
        !(is_root_agent && matrix_root.is_some()),
        "root agent must not also have an origin matrix (single item-3 invariant)"
    );

    // #923 D1: the Root Agent may read and write every agent's replica under a
    // registered project (ROOT_PROJECT_SCOPE_ENTRY), so
    // the peer-replica prohibition is rendered for non-root agents only.
    let replica_usage = if is_root_agent {
        "   Use this for replica-local scratch, inbox/outbox, and session artifacts. Do NOT store \
            canonical memory, plans, or skills here."
            .to_string()
    } else {
        "   Use this for replica-local scratch, inbox/outbox, and session artifacts. Do NOT store \
            canonical memory, plans, or skills here. Do NOT read or write into other agents' \
            replica directories."
            .to_string()
    };
    enum MessagingContextMode {
        None,
        Workgroup(String),
        Root(String),
    }

    let matrix_section = match matrix_root {
        Some(matrix_root) => format!(
            "3. **Your origin Agent Matrix, but only for the canonical agent state listed \
                below:**\n   ```\n   {matrix_root}\n   ```\n   Read-only there: every rotated \
                `memory-archive/memory_YYYYMMDD_hhmmss/` archive of your own memory and legacy \
                root `memory_YYYYMMDD_hhmmss/` archives. You may list your own `memory-archive/` \
                solely to discover archive directories. Read them freely; never write, modify or \
                delete archives or their container.\n   Allowed for reading and writing there:\n   \
                - `memory/`\n   - `plans/`\n   - `skills/`\n   - `Role.md`\n\n",
            matrix_root = matrix_root,
        ),
        None => String::new(),
    };
    // Invariant: messaging stays name-gated (is_root_agent_dir_name) only because
    // the `send` / `list-peers` backends are independently path-gated
    // (is_root_agent_path at cli/send.rs:218, cli/list_peers.rs:1132). If
    // messaging ever trusts this prompt text alone, re-gate it on the path check.
    let messaging_mode = if super::super::root_agent::is_root_agent_dir_name(agent_root) {
        MessagingContextMode::Root(display_path(
            &std::path::Path::new(agent_root).join(crate::phone::messaging::MESSAGING_DIR_NAME),
        ))
    } else {
        match crate::phone::messaging::workgroup_root(std::path::Path::new(agent_root)) {
            Ok(wg) => MessagingContextMode::Workgroup(display_path(
                &wg.join(crate::phone::messaging::MESSAGING_DIR_NAME),
            )),
            Err(_) => MessagingContextMode::None,
        }
    };
    let workgroup_messaging_entry = match &messaging_mode {
        MessagingContextMode::Workgroup(path) => Some(format!(
            concat!(
                r#"4. **Messaging access:**

You MAY create message files inside this directory:

```
{path}
```

Strictly limited to canonical inter-agent message files whose name matches the pattern "#,
                r#"`YYYYMMDD-HHMMSS-<roomN>-<you>-to-<roomN>-<peer>-<slug>.md` (the CLI rejects "#,
                r#"any other shape). Do NOT modify or delete any message file once written. "#,
                r#"Do NOT write any other kind of file here. You may also READ message "#,
                r#"files inside this directory, and list your room root "#,
                r#"(`room-<N>-*`) to resolve that directory's path.

You MAY also READ exactly one specifically identified canonical inter-agent message file inside "#,
                r#"another workspace or room's `messaging` directory, even "#,
                r#"when its path is outside your own workspace or room, "#,
                r#"only when either the user directly instructs you "#,
                r#"in the active conversation to read or process "#,
                r#"that file, or AgentsCommander provides "#,
                r#"the canonical `[Message from <peer>] "#,
                r#"Process this inter-agent message: "#,
                r#"<path>` notification. The "#,
                r#"instruction or "#,
                r#"notification must "#,
                r#"provide that "#,
                r#"exact file "#,
                r#"path without directory discovery, and the filename must match the canonical "#,
                r#"pattern above. This authorization is read-only and limited to that file. "#,
                r#"Do NOT list or explore the other workspace or room, read a second "#,
                r#"external file or any other file or content there, or create, "#,
                r#"write, modify, or delete anything in its `messaging` directory.

"#
            ),
            path = path,
        )),
        MessagingContextMode::Root(_) | MessagingContextMode::None => None,
    };
    let messaging_exception = match &messaging_mode {
        MessagingContextMode::Workgroup(_) => String::new(),
        MessagingContextMode::Root(path) => format!(
            "**Narrow exception — Root Agent messaging directory:**\n\n\
             You MAY create message files inside this directory:\n\n\
             ```\n\
             {path}\n\
             ```\n\n\
             Strictly limited to canonical Root Agent inter-agent message files whose name matches \
             the pattern `YYYYMMDD-HHMMSS-root-to-<roomN>-<orchestrator>-<slug>.md` (the CLI \
             rejects any other shape). Do NOT modify or delete any message file once written. Do \
             NOT write any other kind of file here. You may also READ message files inside this \
             directory.\n\n",
            path = path,
        ),
        MessagingContextMode::None => String::new(),
    };
    let messaging_allowed = match &messaging_mode {
        // Workgroup messaging renders in the standard entry-4 block and Root
        // messaging remains in its Narrow exception; neither uses this fine bullet.
        MessagingContextMode::Workgroup(_) | MessagingContextMode::Root(_) => String::new(),
        // #923 D3: this session has no messaging directory of its own (no `wg-<N>-*`
        // ancestor, not the Root Agent), so `send --send` rejects it outright
        // (cli/send.rs:406). It can still be a delivery target, and the Inter-Agent
        // Messaging block tells it to read the notified path. Grant exactly that read,
        // and nothing more, so the document never orders an operation it forbids.
        MessagingContextMode::None => {
            "- **Allowed (read-only)**: Read an inter-agent message file \
            when AgentsCommander hands you its absolute path in an incoming `[Message from \
            <peer>]` notification. This grant covers that file only; no other path outside the \
            entries above becomes readable.
"
            .to_string()
        }
    };
    let has_messaging_exception = matches!(&messaging_mode, MessagingContextMode::Root(_));
    let ac_root_phrase = if has_messaging_exception {
        "the workspace root (other than the narrow messaging exception above)"
    } else {
        "the workspace root"
    };
    let forbidden_scope = if is_root_agent {
        "the entries listed above; your write scope already covers every registered project folder \
            in `projectPaths` in `project-paths.json`, so the only writes off-limits are the \
            global `settings.json`, `project-paths.json`, the Agency template cache, and any other \
            file anywhere under the app config directory outside your own Root Agent home \
            (CLI-managed, even when the app config directory falls within a registered project \
            folder), plus anything outside the registered set: files of projects not listed in \
            `projectPaths` in `project-paths.json`, user home files unrelated to AgentsCommander, \
            and arbitrary paths on disk"
            .to_string()
    } else if matrix_root.is_some() {
        format!(
            "the entries listed above — including other agents' replica directories, any other \
                files inside the Agent Matrix, {ws}, parent project dirs, user home files, or \
                arbitrary paths on disk",
            ws = ac_root_phrase,
        )
    } else {
        format!(
            "the entries listed above — including other agents' replica directories, {ws}, parent \
                project dirs, user home files, or arbitrary paths on disk",
            ws = ac_root_phrase,
        )
    };
    // Root and None retain fine-grained messaging grants outside the numbered entries.
    // Workgroup messaging is its normal entry 4, so it has no exception carve-out.
    let messaging_read_phrase = match &messaging_mode {
        MessagingContextMode::None => " (other than the inbound message file grant above)",
        MessagingContextMode::Root(_) => " (other than the narrow messaging exception above)",
        MessagingContextMode::Workgroup(_) => "",
    };
    // #923: reads are now restricted to the same allowed entries as writes. The Root
    // Agent already holds a project-wide read grant, so it gets a scope sentence rather
    // than the peer-privacy clause that applies to every other agent. D2: that scope is
    // defined by `project-paths.json`, which lives in the app config dir OUTSIDE every
    // registered project in a normal install, so reading it must be granted explicitly
    // or the grant becomes self-referentially unreadable.
    let forbidden_read_scope = if is_root_agent {
        format!(
            "the entries listed above{ms}, except for explicitly requested AgentsCommander CLI \
                operations covered by the exception below. Your Root Agent scope already grants \
                reads across every project folder registered in `projectPaths` in \
                `project-paths.json`, including its `.ac` tree. You may ALWAYS read the app config \
                `project-paths.json` to enumerate that set, the app config `settings.json` for \
                unrelated settings, and the Agency template cache directory that `agency-templates \
                status` and `agency-templates list` report on; these reads are grants, while \
                direct writes to them stay CLI-managed. Reads stay off-limits beyond the \
                registered set: files of projects not listed in `projectPaths` in \
                `project-paths.json`, user home files unrelated to AgentsCommander, and arbitrary \
                paths on disk.",
            ms = messaging_read_phrase,
        )
    } else {
        format!(
            "the entries listed above{ms}, except for explicitly requested AgentsCommander CLI \
                operations covered by the exception below. This includes other agents' replica \
                directories, and any other agent's `memory*` directories (the live `memory/`, \
                `memory-archive/` and legacy `memory_YYYYMMDD_hhmmss/`), `plans/`, `skills/`, or \
                `Role.md`: another agent's memory is private whether it is live or rotated; do not \
                read, list, search, or summarize it, even if asked. If you need information \
                another agent holds, message that agent and ask.",
            ms = messaging_read_phrase,
        )
    };
    let git_scope = if is_root_agent {
        ROOT_GIT_SCOPE
    } else if matrix_root.is_some() {
        WORKGROUP_GIT_SCOPE
    } else {
        DIRECT_MATRIX_GIT_SCOPE
    }
    .to_string();
    let peer_name_format = match &messaging_mode {
        MessagingContextMode::Root(_) => "- **Root Agent sessions**: verified Room orchestrator \
            replicas only, shaped `<project>:<room>/<agent>`, e.g. \
            `agentscommander:room-15-dev-team/tech-lead`. Origin orchestrators and \
            non-orchestrator Room replicas are not valid Root Agent targets in #277."
            .to_string(),
        _ => "- **Room replicas** (the common case): `<project>:<room>/<agent>`, e.g. \
            `agentscommander:room-15-dev-team/dev-rust`.\n- **Origin agents**: `<project>/\
            <agent>`, e.g. `agentscommander/architect`."
            .to_string(),
    };
    let agency_cache_guidance = root_agency_cache_guidance(agent_root);
    let send_message_instructions = match &messaging_mode {
        MessagingContextMode::Root(path) => format!(
            "Use only the JSON `name` values returned by `list-peers-lean`; Root sessions list \
                verified Room orchestrator replicas only.\n\n\
             Root messaging is **file-based** to avoid PTY truncation:\n\n\
             1. Write a new file in the Root Agent messaging directory:\n\n\
             ```\n\
             {path}\n\
             ```\n\n\
             Name it `YYYYMMDD-HHMMSS-root-to-<roomN>-<orchestrator>-<slug>.md` (legacy: `<wgN>`) \
             (UTC, sanitized kebab-case slug \u{2264}50 chars).\n\
             2. Send:\n\n\
             ```\n\
             \"<AGENTSCOMMANDER_BINARY_PATH>\" send --token <AGENTSCOMMANDER_TOKEN> --root \
             \"<AGENTSCOMMANDER_ROOT>\" --to \"<orchestrator_name>\" --send <filename> --mode \
             wake\n\
             ```\n\n\
             `--send` takes the filename ONLY, never a path.\n",
            path = path,
        ),
        MessagingContextMode::Workgroup(_) => "Messages are **file-based** to avoid PTY \
            truncation:\n\n\
             1. Write a new file in `<room-root>/messaging/` (walk up from your root to the parent \
             `room-<N>-*` folder (or `wg-<N>-*` if legacy)). Name it \
             `YYYYMMDD-HHMMSS-<roomN>-<you>-to-<roomN>-<peer>-<slug>.md` (legacy: `<wgN>`) (UTC, \
             sanitized kebab-case slug \u{2264}50 chars).\n\
             2. Send:\n\n\
             ```\n\
             \"<AGENTSCOMMANDER_BINARY_PATH>\" send --token <AGENTSCOMMANDER_TOKEN> --root \
             \"<AGENTSCOMMANDER_ROOT>\" --to \"<agent_name>\" --send <filename> --mode wake\n\
             ```\n\n\
             `--send` takes the filename ONLY, never a path.\n"
            .to_string(),
        // #923 D3: no `wg-<N>-*` ancestor and not the Root Agent, so `send --send`
        // refuses this root (cli/send.rs:406-414). Telling it to walk up to a workgroup
        // root it does not have would order an operation the Golden Rule forbids and the
        // CLI rejects. State the truth instead.
        MessagingContextMode::None => "This session has no messaging directory: `--send` requires \
            a `--root` under `room-<N>-*` or `wg-<N>-*` or the canonical Root Agent directory. Do \
            NOT search the filesystem for one. You can still receive messages: when an incoming \
            `[Message from <peer>]` notification gives an absolute path, read that file, act, and \
            report here instead of using `send --send`.\n"
            .to_string(),
    };

    let (root_scope_section, root_authority_section) = if is_root_agent {
        (
            ROOT_PROJECT_SCOPE_ENTRY.to_string(),
            ROOT_AUTHORITY_SECTION.to_string(),
        )
    } else {
        (String::new(), String::new())
    };

    // #1795: both entries come from ONE resolution of the agent root, so they are
    // empty together or populated together. No `debug_assert!` that entry 4 implies
    // entries 5 and 6: under the strict resolver a nested non-replica renders entry 4
    // and must NOT render entries 5 and 6, so that implication is false by design.
    let (project_shared_entry, room_shared_entry) = match replica_shared_roots(agent_root) {
        Some((ac_root, room_root)) => (
            render_project_shared_entry(&ac_root),
            render_room_shared_entry(&room_root),
        ),
        None => (String::new(), String::new()),
    };
    DefaultContextDynamicValues {
        replica_usage,
        matrix_section,
        workgroup_messaging_entry,
        project_shared_entry,
        room_shared_entry,
        messaging_exception,
        messaging_allowed,
        forbidden_scope,
        forbidden_read_scope,
        git_scope,
        agency_cache_guidance,
        peer_name_format,
        send_message_instructions,
        root_scope_section,
        root_authority_section,
    }
}

pub(crate) fn root_agency_cache_guidance(agent_root: &str) -> String {
    if !super::super::root_agent::is_root_agent_dir_name(agent_root) {
        return String::new();
    }
    let cache_path = std::path::Path::new(agent_root)
        .parent()
        .map(|p| p.join(crate::commands::role_templates::AGENCY_TEMPLATES_DIR))
        .unwrap_or_else(|| {
            std::path::PathBuf::from(crate::commands::role_templates::AGENCY_TEMPLATES_DIR)
        });
    format!(
        "Root Agent Agency template cache: `{}`. Manage it only through the documented \
            `agency-templates update`, `agency-templates status`, and `agency-templates list` CLI \
            commands. This does not grant direct shell writes to the cache, nor access to \
            arbitrary `*_templates` paths.\n\n",
        display_path(&cache_path)
    )
}

#[cfg(test)]
pub(crate) fn default_context(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
) -> String {
    render_default_agent_context(
        agent_root,
        matrix_root,
        skills_section,
        Path::new(agent_root),
        None,
        None,
    )
}

/// #979 G.2: Root no longer reaches the global-template renderer, so every Root
/// governance test that rides this helper must exercise the code-owned prologue.
/// Left pointed at `render_agent_context_template_inner`, nine of them would keep
/// passing while asserting nothing about Root's real prompt. Tests that
/// intentionally exercise the legacy global fallback call
/// `render_agent_context_template_inner` explicitly instead.
#[cfg(test)]
pub(crate) fn default_context_as_root(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
) -> String {
    debug_assert!(
        matrix_root.is_none(),
        "a root agent has no origin matrix (single item-3 invariant)"
    );
    render_root_runtime_prologue_inner(
        agent_root,
        skills_section,
        Path::new(agent_root),
        None,
        None,
        true,
    )
}
