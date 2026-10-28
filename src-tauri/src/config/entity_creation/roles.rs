//! App-created Role.md bytes and rendering; excludes template resolution.
use crate::filesystem::projects::entities::publication;
use std::path::{Path, PathBuf};

pub(crate) struct RoleProfileRef<'a> {
    pub id: &'a str,
    pub body: &'a str,
}

pub(crate) fn write_created_role(root: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = root.join("Role.md");
    publication::write_created_role(&path, bytes)
        .map_err(|e| format!("Failed to write Role.md: {}", e))?;
    Ok(path)
}

/// Build the Role.md body written by `create_agent_matrix`. Kept as a separate
/// pure helper so the legacy (no-template) format can be unit-tested without
/// touching the Tauri command surface. `template` is the resolved template; when
/// `None`, output is byte-identical to the pre-#271 inline `format!`.
///
/// When a template is supplied, its body is inserted as a `## Role Profile`
/// section between the title/description and the mandatory `## Source of Truth`
/// / `## Agent Memory Rule` sections (plan §7) — the mandatory sections always
/// land last so a template body cannot push them off, and Role-Profile content
/// inherits the picker's display order (the section header makes the source of
/// the imported text obvious to a human reader of Role.md).
pub(crate) fn build_role_content(
    safe_name: &str,
    description: &str,
    template: Option<&RoleProfileRef<'_>>,
) -> String {
    let desc_yaml = description.replace('\'', "''");
    // Plan §7.2: imported template body is fenced by HTML-comment delimiters so
    // the section boundary stays machine-detectable (and the opening tag
    // records provenance via the template id). Pre-release maintainer review
    // (§10.2) screens template bodies for the literal closing delimiter to
    // prevent a body from "escaping" the fenced section.
    let profile = match template {
        Some(t) => format!(
            "\n## Role Profile\n\n\
             <!-- ac:role-profile source=\"{}\" — imported template body; \
             the AC sections below are mandatory and must stay last -->\n\n\
             {}\n\n\
             <!-- ac:role-profile:end -->\n",
            t.id,
            t.body.trim(),
        ),
        None => String::new(),
    };
    format!(
        "---\nname: '{name}'\ndescription: '{desc_yaml}'\ntype: agent\n---\n\n\
         # {name}\n\n\
         {description}\n\
         {profile}\n\
         ## Source of Truth\n\n\
         This role is defined in Role.md of your Agent Matrix at: .ac/_agent_{name}/\n\
         If you are running as a replica, this file was generated from that source; the \
         Agent Matrix is canonical.\n\n\
         ## Agent Memory Rule\n\n\
         Your Agent Matrix's memory/, plans/, skills/, and Role.md are the single source \
         of persistent knowledge. Use your replica folder only for replica-local scratch, \
         inbox/outbox, and session artifacts. NEVER use external memory systems from the \
         coding agent (e.g., ~/.claude/projects/memory/).\n",
        name = safe_name,
        desc_yaml = desc_yaml,
        description = description,
        profile = profile,
    )
}

#[cfg(test)]
mod tests;
