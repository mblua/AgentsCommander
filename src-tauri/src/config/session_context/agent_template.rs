use super::{
    classify_legacy_rendered_default_context, coarse_section_dedup_safe,
    default_context_dynamic_values, heal_stale_global_recorded, mandatory_placeholder_aliases,
    mandatory_section_present_inline, read_context_local_override, read_or_create_context_recorded,
    render_agent_repos_string, render_host_platform_rules_block,
    render_inter_agent_messaging_block, render_write_restrictions_block,
    LegacyRenderedDefaultContext, TemplateTokenSignals, DEFAULT_CLI_CONTEXT,
    DEFAULT_DELEGATED_TASK_REPORTING, DEFAULT_SESSION_CREDENTIALS,
    GLOBAL_CONTEXT_TEMPLATE_FILENAME, LEGACY_AGENT_REPOS_PLACEHOLDER,
    MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS,
};
use std::path::Path;

pub fn get_default_agent_template() -> &'static str {
    concat!(
        r#"# AgentsCommander Context

You are in AgentsCommander, a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and "#,
        r#"which repos are available.
- **Room**: a runtime replica of a team for a specific task. It contains replica agents and "#,
        r#"`repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{HOST_PLATFORM_RULES}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}

## Answering

Draft, count words, then rewrite to half or fewer; report both counts. Paths, commands, digests, "#,
        r#"figures, and quoted evidence are exempt: not counted, not cut. Never cut a "#,
        r#"fact to reach half; stop at the smallest size that keeps every fact and "#,
        r#"say so.

Use plain, easy language.
"#
    )
}

pub(crate) const PTY_INPUT_COORDINATOR_CONTEXT: &str = concat!(
    r#"## Privileged PTY Input

This capability is present only because this session is an identity-verified room orchestrator "#,
    r#"replica. You may ask AgentsCommander to submit validated text to exactly one "#,
    r#"non-orchestrator member in this same project and room. Resolve the exact target with "#,
    r#"`list-peers-lean`. This writes text into the target coding-agent PTY; it never "#,
    r#"directly executes a host or container OS shell command.

Local host session:
"<AGENTSCOMMANDER_BINARY_PATH>" send --token <AGENTSCOMMANDER_TOKEN> --root "#,
    r#""<AGENTSCOMMANDER_ROOT>" --to "<agent_name>" --pty-input-stdin --mode wake

Container session with `AGENTSCOMMANDER_TRANSPORT=api`:
"<AGENTSCOMMANDER_BINARY_PATH>" send --to "<agent_name>" --pty-input-stdin --mode wake
"<AGENTSCOMMANDER_BINARY_PATH>" pty-input-status --op-id "<operation_id>"

Prefer stdin for multiline or sensitive text. `Queued` is not `Injected`. If confirmation times "#,
    r#"out, keep the reported operation ID and inspect its status; do not "#,
    r#"create a new operation to retry it."#
);

pub fn get_default_coordinator_template() -> &'static str {
    "You are the orchestrator for your team. You must:\n\
     - Keep your base role; coordination is an additional assignment, not a replacement.\n\
     - Receive team work requests and clarify scope, outcome, constraints, and acceptance \
     criteria.\n\
     - Route each part of a request to the team member best prepared for it by role, skills, and \
     current assignment; delegate instead of absorbing technical work when a more specialized \
     agent is available.\n\
     - To reach another room, message its orchestrator, never its members, and only when your \
     role, the user, or the Root Agent authorizes it; replying to an orchestrator who messaged you \
     first is always authorized.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working; contact \
     silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and \
     verification before treating delegated work as complete; never infer completion solely from \
     files/logs/artifacts/status flags when the agent has not reported the outcome.\n\
     - Give recommendations that help an agent work better without removing or overriding that \
     agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label \
         <LABEL>]\n\
     --path is required; --caption is optional, max 1024 UTF-16 units. If multiple Telegram bots \
     are configured, pick one with --bot-id or --bot-label. jpg/jpeg/png/webp up to 10 MB use \
     sendPhoto; other formats including GIF use sendDocument up to 50 MB. Symlinks/junctions are \
     rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop orchestrator: PowerShell System.Drawing / CopyFromScreen can work; cast \
     Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness orchestrator: CopyFromScreen may return all-zero/black pixels; then ask \
     the user to capture with Greenshot, use the latest file from C:\\Users\\maria\\0_greenshot\\, \
     and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \
         \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your orchestrator row; it clears when the \
     user interacts with your session.\n"
}

pub(crate) fn render_agent_context_template(
    template: &str,
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
    cwd_path: &Path,
    config: Option<&serde_json::Value>,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
) -> String {
    let is_root_agent = super::super::root_agent::is_root_agent_path(agent_root);
    render_agent_context_template_inner(
        template,
        agent_root,
        matrix_root,
        skills_section,
        cwd_path,
        config,
        repo_mounts,
        is_root_agent,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_agent_context_template_inner(
    template: &str,
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
    cwd_path: &Path,
    config: Option<&serde_json::Value>,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    is_root_agent: bool,
) -> String {
    let rendered =
        default_context_dynamic_values(agent_root, matrix_root, skills_section, is_root_agent);
    let mut template = template.to_string();
    let signals = TemplateTokenSignals::capture(&template);
    for placeholder in MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS {
        if template.contains(placeholder)
            || mandatory_placeholder_aliases(placeholder)
                .iter()
                .any(|alias| template.contains(alias))
        {
            // Token (or a frozen legacy spelling of it) present: the replace
            // chain below fills it in place.
            continue;
        }
        // #658: a legacy template wrote this section's prose without its coarse
        // token. Skip the append (dedup) only when the inline heading is present
        // AND deduping is SAFE for this agent/template shape (the per-token
        // dedup-safety invariant). coarse_section_dedup_safe is false for the
        // Root Golden Rule (HIGH-1), for a baked legacy inline copy (stale
        // foreign paths), and for skills/workspace lists; in those cases we fall
        // through and append the CURRENT block instead of trusting the inline.
        if mandatory_section_present_inline(&template, placeholder)
            && coarse_section_dedup_safe(placeholder, is_root_agent, &signals)
        {
            log::debug!(
                "Global context template lacks placeholder {} but its section is present inline \
                    and safe to dedup; skipping fallback append to avoid duplication",
                placeholder
            );
            continue;
        }
        // Genuinely-absent section, OR an inline copy that is not safe to trust
        // (Root Golden Rule / baked legacy / dynamic list): append the fallback
        // so the agent receives this mandatory governance with current content.
        log::warn!(
            "Global context template is missing mandatory placeholder {}; appending fallback block",
            placeholder
        );
        template.push_str("\n\n");
        template.push_str(placeholder);
    }

    let agent_repos = render_agent_repos_string(cwd_path, config, repo_mounts, is_root_agent);
    // #1605: computed BEFORE the replace chain (like `agent_repos`): the value
    // feeds the `{{HOST_PLATFORM_RULES}}` replace between `{{CLI_CONTEXT}}` and
    // `{{SESSION_CREDENTIALS}}`. Container sessions (repo_mounts.is_some())
    // render nothing.
    let host_platform_rules = render_host_platform_rules_block(agent_root, repo_mounts);
    // #1369 (C4): a template may carry the current token, the frozen legacy
    // alias, or - pathologically, if a user pasted both - the two of them.
    // Exactly one block must be emitted, so the alias renders the block only
    // when the current token is absent. Computed AFTER the mandatory loop,
    // because that loop may have appended the current token.
    //
    // This guard is LOAD-BEARING and must never be deleted as dead defensive
    // code: it is the single reason the both-tokens case cannot emit two
    // blocks. Removing it while keeping the `mandatory_placeholder_aliases`
    // arm renders TWO `# Agent Repos` blocks with no raw placeholder left, so
    // `assert_no_raw_template_placeholders` stays green and the only test in
    // the suite that notices is
    // `agent_repos_both_tokens_render_exactly_one_block` (AC-2.3).
    let legacy_repos_replacement = if template.contains("{{AGENT_REPOS}}") {
        ""
    } else {
        agent_repos.as_str()
    };

    template
        .replace("{{AGENT_ROOT}}", agent_root)
        .replace("{{MATRIX_SECTION}}", &rendered.matrix_section)
        // #1005 S3: the Allowed-bullet family is gone; a legacy hybrid template
        // carrying this fine-grained token renders nothing there (the grant's
        // single carrier is entry 3 inside {{MATRIX_SECTION}}).
        .replace("{{MATRIX_ALLOWED}}", "")
        .replace("{{MESSAGING_EXCEPTION}}", &rendered.messaging_exception)
        .replace("{{MESSAGING_ALLOWED}}", &rendered.messaging_allowed)
        .replace("{{FORBIDDEN_SCOPE}}", &rendered.forbidden_scope)
        .replace("{{GIT_SCOPE}}", &rendered.git_scope)
        .replace("{{PEER_NAME_FORMAT}}", &rendered.peer_name_format)
        .replace(
            "{{SEND_MESSAGE_INSTRUCTIONS}}",
            &rendered.send_message_instructions,
        )
        .replace("{{SKILLS_SECTION}}", skills_section)
        .replace(
            "{{WRITE_RESTRICTIONS}}",
            &render_write_restrictions_block(agent_root, &rendered),
        )
        .replace("{{CLI_CONTEXT}}", DEFAULT_CLI_CONTEXT)
        .replace("{{HOST_PLATFORM_RULES}}", &host_platform_rules)
        .replace("{{SESSION_CREDENTIALS}}", DEFAULT_SESSION_CREDENTIALS)
        .replace(
            "{{INTER_AGENT_MESSAGING}}",
            &render_inter_agent_messaging_block(&rendered),
        )
        .replace(LEGACY_AGENT_REPOS_PLACEHOLDER, legacy_repos_replacement)
        .replace("{{AGENT_REPOS}}", &agent_repos)
        .replace(
            "{{DELEGATED_TASK_REPORTING}}",
            DEFAULT_DELEGATED_TASK_REPORTING,
        )
}

pub(crate) fn render_default_agent_context(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
    cwd_path: &Path,
    config: Option<&serde_json::Value>,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
) -> String {
    render_agent_context_template(
        get_default_agent_template(),
        agent_root,
        matrix_root,
        skills_section,
        cwd_path,
        config,
        repo_mounts,
    )
}

#[cfg(test)]
pub(crate) fn resolve_agent_context(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
    cwd_path: &Path,
    config: Option<&serde_json::Value>,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
) -> Result<String, String> {
    resolve_agent_context_with_activation(
        agent_root,
        matrix_root,
        skills_section,
        cwd_path,
        config,
        repo_mounts,
        None,
    )
}

pub(crate) fn resolve_agent_context_with_activation(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
    cwd_path: &Path,
    config: Option<&serde_json::Value>,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<String, String> {
    // #979 load-bearing guard 2 of 2. This MUST remain the FIRST executable
    // statement of this function. It sits ahead of `read_or_create_context_template`
    // below, which SYNCS (`sync_project_context_template_for_read`) and CREATES
    // (`write_template_if_missing`) the resolved global, and ahead of the
    // `StaleGenerated` arm, which atomically REWRITES it through
    // `heal_stale_global_context_template`.
    //
    // In the dev and workgroup layouts `config_dir()` sits INSIDE a `.ac` tree, so
    // `find_ac_root` resolves the *project's* `Context.AgentsCommander.md`
    // for a Root-named path: moving this guard below the template read "where it
    // reads better" silently reopens a project-file create-and-rewrite. Root's
    // context comes from `render_root_runtime_prologue` instead. In production this
    // function is only ever called with `canonical_root` (:58-59), and the
    // cache-writer branch tests both basenames, so the two guards cannot disagree.
    if super::super::root_agent::is_root_agent_dir_name(agent_root) {
        return Err(format!(
            "Root agent directory {} must not resolve the global context template; use the \
                dedicated Root builder",
            agent_root
        ));
    }

    let template = read_or_create_context_recorded(
        agent_root,
        GLOBAL_CONTEXT_TEMPLATE_FILENAME,
        get_default_agent_template(),
        activation,
    )?
    .unwrap_or_else(|| get_default_agent_template().to_string());

    // #1737 (D1, D3): the classification runs on the BASE template and keeps every
    // one of its side effects, including the `StaleGenerated` self-heal of the base
    // file. Its value is bound rather than returned, and an override then decides the
    // content handed to the session, always as a NON-legacy template: an
    // operator-authored file is by definition not a generated legacy default, and
    // making the outcome independent of the base file's classification is what makes
    // the override behave the same on every machine.
    let base_result = match classify_legacy_rendered_default_context(
        &template,
        agent_root,
        matrix_root,
        skills_section,
    ) {
        LegacyRenderedDefaultContext::Current => Ok(template),
        LegacyRenderedDefaultContext::StaleGenerated => {
            let execution =
                heal_stale_global_recorded(agent_root, matrix_root, skills_section, activation);
            if let Some(publication) = execution.published {
                log::debug!(
                    "#664 self-heal: {} published at {}",
                    GLOBAL_CONTEXT_TEMPLATE_FILENAME,
                    publication.published_at
                );
            }
            if let Err(error) = &execution.completion {
                log::warn!("#664 self-heal: {}", error);
            }
            Ok(render_default_agent_context(
                agent_root,
                matrix_root,
                skills_section,
                cwd_path,
                config,
                repo_mounts,
            ))
        }
        LegacyRenderedDefaultContext::NotLegacy => Ok(render_agent_context_template(
            &template,
            agent_root,
            matrix_root,
            skills_section,
            cwd_path,
            config,
            repo_mounts,
        )),
    };
    match read_context_local_override(agent_root, GLOBAL_CONTEXT_TEMPLATE_FILENAME) {
        Some(local) => Ok(render_agent_context_template(
            &local,
            agent_root,
            matrix_root,
            skills_section,
            cwd_path,
            config,
            repo_mounts,
        )),
        None => base_result,
    }
}
