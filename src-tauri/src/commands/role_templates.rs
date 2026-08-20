//! Role-template picker backend (#271).
//!
//! Exposes cached Agency and local role templates used by the New Agent dialog,
//! plus the resolution helpers `create_agent_matrix` calls to seed a new Agent
//! Matrix's `Role.md` (and optionally its `skills/`).
//!
//! No prompt body crosses the IPC boundary: `list_role_templates` returns
//! metadata only (`RoleTemplateMeta`); the frontend sends back only an `id`,
//! and the body is read locally here (Agency cache or local file).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::instance_artifacts::AGENT_TEMPLATES_DIR_NAME;

// ---------------------------------------------------------------------------
// IPC type — metadata only (no prompt body)
// ---------------------------------------------------------------------------

/// Metadata for one role template. Mirrored by the TypeScript `RoleTemplateMeta`
/// interface in `src/shared/types.ts`. Deliberately carries NO prompt body —
/// see the module docs and plan §8.6.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleTemplateMeta {
    /// Source-qualified id: "agency:<file-stem>" or "local:<folder-name>".
    pub id: String,
    /// "agency" | "local" — machine-readable source.
    pub source: String,
    /// Display name (frontmatter `name`, else folder/file stem).
    pub name: String,
    /// Short one-line description (frontmatter `description`, may be empty).
    pub description: String,
    /// Display grouping label: title-cased division ("Engineering") for agency,
    /// "Local" for local templates.
    pub category: String,
    /// Optional accent color (agency frontmatter `color`); None for local v1.
    pub color: Option<String>,
    /// Optional emoji; reserved/forward-compat — None for both sources in v1.
    pub emoji: Option<String>,
    /// True iff the template ships a non-empty `skills/` directory.
    pub has_skills: bool,
}

// ---------------------------------------------------------------------------
// Agency runtime cache
// ---------------------------------------------------------------------------

pub const AGENCY_TEMPLATES_DIR: &str = crate::config::instance_artifacts::AGENCY_TEMPLATES_DIR;
pub const AGENCY_MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgencyTemplatesManifest {
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub commit: String,
    pub template_count: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgencyTemplatesStatus {
    pub available: bool,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AgencyTemplate {
    id: String,
    name: String,
    description: String,
    category: String,
    color: Option<String>,
    body: String,
    #[allow(dead_code)]
    role_path: PathBuf,
}

pub fn agency_templates_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(AGENCY_TEMPLATES_DIR)
}

pub fn agency_manifest_path(config_dir: &Path) -> PathBuf {
    agency_templates_dir(config_dir).join(AGENCY_MANIFEST_FILE)
}

// ---------------------------------------------------------------------------
// Resolved template (data `create_agent_matrix` needs)
// ---------------------------------------------------------------------------

/// A template resolved to the data `create_agent_matrix` needs. Body is always
/// frontmatter-free (agency bodies are pre-stripped; local bodies stripped here).
#[derive(Debug)]
pub struct ResolvedRoleTemplate {
    pub id: String,
    pub body: String,
    /// `Some(<template>/skills)` for a local template that ships skills.
    pub skills_src: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Local templates path resolution
// ---------------------------------------------------------------------------

/// Outcome of resolving `settings.agent_templates_path`.
#[derive(Debug)]
pub enum LocalTemplatesDir {
    /// Default path (`<config_dir>/agent-templates`). Always ensured to exist.
    Default(PathBuf),
    /// A valid explicit override.
    Custom(PathBuf),
    /// An explicit override that is missing / not a directory. The caller
    /// surfaces this via `log::error!` (→ ErrorModal); built-ins still work.
    InvalidCustom {
        configured: String,
        resolved: PathBuf,
    },
}

/// Pure resolver — `config_dir` is a parameter so it is unit-testable.
pub fn resolve_local_templates_dir(
    settings: &crate::config::settings::AppSettings,
    config_dir: &Path,
) -> LocalTemplatesDir {
    let default = config_dir.join(AGENT_TEMPLATES_DIR_NAME);
    let configured = settings
        .agent_templates_path
        .as_deref()
        .map(str::trim)
        .unwrap_or("");
    if configured.is_empty() {
        return LocalTemplatesDir::Default(default);
    }
    let p = Path::new(configured);
    let resolved = if p.is_absolute() {
        p.to_path_buf()
    } else {
        config_dir.join(p)
    };
    if resolved.is_dir() {
        LocalTemplatesDir::Custom(resolved)
    } else {
        LocalTemplatesDir::InvalidCustom {
            configured: configured.to_string(),
            resolved,
        }
    }
}

// ---------------------------------------------------------------------------
// Default-directory seeding
// ---------------------------------------------------------------------------

/// English README seeded into a freshly created `agent-templates/`.
const AGENT_TEMPLATES_README: &str = r#"# Agent Templates

This folder holds **local agent role templates** for AgentsCommander's New Agent
dialog. Each template can pre-fill a new Agent Matrix's `Role.md` and, optionally,
copy a set of `skills/` into it.

## Adding a template

Create one subfolder per template. The subfolder name is the template id, shown
in the picker as `local:<folder-name>`.

    agent-templates/
      my-template/
        Role.md        (required)
        skills/        (optional)

### Role.md (required)

`Role.md` holds the role text. Optional YAML frontmatter at the very top supplies
the picker's display name and description:

    ---
    name: My Template
    description: One-line summary shown in the picker.
    ---

    # Role

    Write the role instructions here. This text is imported into the new agent's
    Role.md inside a `## Role Profile` section.

If the frontmatter is missing, the folder name is used as the display name.

### skills/ (optional)

Every file inside `skills/` is copied into the new agent's `skills/` folder when
the template is selected.

## Notes

- This folder is created automatically. You may delete it; it is recreated
  (empty, with this README) on the next launch.
- To use a different folder, set `agentTemplatesPath` in `settings.json`, which
  lives in the application's config folder next to the app executable. A relative
  value is resolved against that config folder; an absolute value is used as-is.
- Agency templates are available only after the explicit CLI cache is downloaded.
"#;

/// Create `<config_dir>/agent-templates/` + a default `README.md` if missing.
/// Idempotent: an existing dir is left alone; an existing README is NEVER
/// overwritten. `config_dir` is a parameter for testability.
pub fn ensure_default_templates_dir(config_dir: &Path) -> Result<(), String> {
    let dir = config_dir.join(AGENT_TEMPLATES_DIR_NAME);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create agent-templates dir: {}", e))?;
    let readme = dir.join("README.md");
    if !readme.exists() {
        std::fs::write(&readme, AGENT_TEMPLATES_README)
            .map_err(|e| format!("Failed to write agent-templates README.md: {}", e))?;
    }
    Ok(())
}

/// Startup convenience wrapper — resolves the real config dir and logs (warn)
/// on failure. A missing default dir is never fatal.
pub fn ensure_default_templates_dir_at_config() {
    match crate::config::config_dir() {
        Some(d) => {
            if let Err(e) = ensure_default_templates_dir(&d) {
                log::warn!(
                    "[role-templates] could not seed default agent-templates dir: {}",
                    e
                );
            }
        }
        None => log::warn!("[role-templates] cannot resolve config dir; skipped seeding"),
    }
}

// ---------------------------------------------------------------------------
// Frontmatter helpers (self-contained — module decoupling)
// ---------------------------------------------------------------------------

/// Extract `name` + `description` from leading YAML frontmatter. Returns
/// (None, None) when there is no frontmatter. Mirrors
/// `entity_creation::parse_role_frontmatter`; duplicated deliberately to keep
/// the two command modules decoupled. Strips a leading UTF-8 BOM first so a
/// `Role.md` saved as UTF-8-with-BOM still yields its display name/description.
fn parse_template_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !content.starts_with("---") {
        return (None, None);
    }
    let rest = &content[3..];
    let end = match rest.find("---") {
        Some(i) => i,
        None => return (None, None),
    };
    let frontmatter = &rest[..end];
    let mut name = None;
    let mut description = None;
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("name:") {
            name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(val) = trimmed.strip_prefix("description:") {
            description = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    (name, description)
}

/// Return `content` with a leading `---\n…\n---` YAML frontmatter block removed.
/// If there is no frontmatter, returns `content` (post-BOM) unchanged.
///
/// Strips a leading UTF-8 BOM FIRST, then treats the input as frontmatter only
/// if it begins with exactly `---` immediately followed by a newline (`---\n`
/// or `---\r\n`) — this avoids mistaking a body that opens with a Markdown
/// `---` horizontal rule for frontmatter. If no closing `---` line is found,
/// returns `content` (post-BOM) unchanged.
fn strip_yaml_frontmatter(content: &str) -> &str {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    let after_open = match content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    {
        Some(rest) => rest,
        None => return content,
    };
    // Scan for a closing `---` line; return the body after it. If there is no
    // closing line, the input was not real frontmatter — return it unchanged.
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed == "---" {
            return &after_open[offset + line.len()..];
        }
        offset += line.len();
    }
    content
}

pub(crate) fn strip_yaml_frontmatter_for_role_template(content: &str) -> &str {
    strip_yaml_frontmatter(content)
}

#[derive(Default)]
pub(crate) struct TemplateFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
}

pub(crate) fn parse_agency_template_frontmatter(content: &str) -> TemplateFrontmatter {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    let Some(after_open) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    else {
        return TemplateFrontmatter::default();
    };
    let mut frontmatter = String::new();
    for line in after_open.lines() {
        if line.trim() == "---" {
            break;
        }
        frontmatter.push_str(line);
        frontmatter.push('\n');
    }
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&frontmatter) else {
        return TemplateFrontmatter::default();
    };
    let scalar = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    };
    TemplateFrontmatter {
        name: scalar("name"),
        description: scalar("description"),
        color: scalar("color").filter(|s| s.len() <= 64 && !s.contains('\n')),
    }
}

pub(crate) fn title_case_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn slug_segment(raw: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let slug = out.trim_matches('-').to_string();
    if slug.is_empty() {
        return Err(format!("Cannot derive a non-empty slug from \"{}\"", raw));
    }
    let reserved = matches!(
        slug.as_str(),
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    );
    if reserved || slug.ends_with('.') || slug.ends_with(' ') {
        return Err(format!(
            "Slug \"{}\" is not safe as a Windows path segment",
            slug
        ));
    }
    Ok(slug)
}

pub fn validate_role_template_body(source_id: &str, body: &str) -> Result<(), String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(format!("Template {} Role.md is empty", source_id));
    }
    for ch in trimmed.chars() {
        if ch == '\0' || (ch.is_control() && ch != '\t' && ch != '\n' && ch != '\r') {
            return Err(format!(
                "Template {} Role.md contains unsupported control characters",
                source_id
            ));
        }
    }
    if trimmed.contains("<!-- ac:role-profile") || trimmed.contains("<!-- /ac:role-profile -->") {
        return Err(format!(
            "Template {} Role.md contains reserved AgentsCommander role delimiters",
            source_id
        ));
    }
    let start = trimmed.trim_start();
    if start.starts_with("# AGENTS.md instructions for")
        || start.starts_with("# AgentsCommander Context")
    {
        return Err(format!(
            "Template {} Role.md contains managed AgentsCommander context",
            source_id
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Link-safety helpers (plan §8.4 — local templates may be authored by others)
// ---------------------------------------------------------------------------

/// True if `meta` describes a symlink or — on Windows — ANY reparse point
/// (junction, mount point, …). A Windows directory junction is a reparse point
/// that `FileType::is_symlink()` does not reliably report, so the raw
/// `FILE_ATTRIBUTE_REPARSE_POINT` (0x400) bit is also checked.
fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// `symlink_metadata` (no link traversal) + reparse-point rejection. `Err(())`
/// means "missing, unreadable, or a link/reparse point" — callers skip/reject.
fn lstat_no_links(path: &Path) -> Result<std::fs::Metadata, ()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if is_link_or_reparse(&m) => Err(()),
        Ok(m) => Ok(m),
        Err(_) => Err(()),
    }
}

// ---------------------------------------------------------------------------
// Agency cache validation + metadata collection
// ---------------------------------------------------------------------------

fn is_valid_commit_sha(commit: &str) -> bool {
    commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn read_agency_manifest(config_dir: &Path) -> Result<Option<AgencyTemplatesManifest>, String> {
    let root = agency_templates_dir(config_dir);
    if !root.exists() {
        return Ok(None);
    }
    let meta = lstat_no_links(&root).map_err(|()| "cacheInvalid".to_string())?;
    if !meta.is_dir() {
        return Err("cacheInvalid".into());
    }
    let manifest_path = agency_manifest_path(config_dir);
    if !manifest_path.exists() {
        return Err("manifestMissing".into());
    }
    let manifest_meta =
        lstat_no_links(&manifest_path).map_err(|()| "manifestMissing".to_string())?;
    if !manifest_meta.is_file() {
        return Err("manifestMissing".into());
    }
    let raw =
        std::fs::read_to_string(&manifest_path).map_err(|_| "manifestMalformed".to_string())?;
    let manifest: AgencyTemplatesManifest =
        serde_json::from_str(&raw).map_err(|_| "manifestMalformed".to_string())?;
    if !is_valid_commit_sha(&manifest.commit) {
        return Err("invalidCommit".into());
    }
    Ok(Some(manifest))
}

pub fn agency_templates_status(config_dir: &Path) -> AgencyTemplatesStatus {
    let path = agency_templates_dir(config_dir)
        .to_string_lossy()
        .to_string();
    let unavailable = |reason: &str| AgencyTemplatesStatus {
        available: false,
        path: path.clone(),
        repo: None,
        reference: None,
        commit: None,
        template_count: None,
        reason: Some(reason.to_string()),
    };

    let manifest = match read_agency_manifest(config_dir) {
        Ok(Some(m)) => m,
        Ok(None) => return unavailable("missing"),
        Err(reason) => return unavailable(&reason),
    };
    match load_cached_agency_templates(config_dir) {
        Ok(templates) if templates.len() == manifest.template_count => AgencyTemplatesStatus {
            available: true,
            path,
            repo: Some(manifest.repo),
            reference: Some(manifest.reference),
            commit: Some(manifest.commit),
            template_count: Some(manifest.template_count),
            reason: None,
        },
        Ok(_) => unavailable("templateCountMismatch"),
        Err(reason) => unavailable(&reason),
    }
}

pub(crate) fn load_cached_agency_templates(
    config_dir: &Path,
) -> Result<Vec<AgencyTemplate>, String> {
    let manifest = read_agency_manifest(config_dir)?.ok_or_else(|| "missing".to_string())?;
    let root = agency_templates_dir(config_dir);
    let templates = collect_agency_templates_from_dir(&root)?;
    if templates.len() != manifest.template_count {
        return Err("templateCountMismatch".into());
    }
    Ok(templates)
}

pub(crate) fn collect_agency_templates_from_dir(
    root: &Path,
) -> Result<Vec<AgencyTemplate>, String> {
    let root_meta = lstat_no_links(root).map_err(|()| "cacheInvalid".to_string())?;
    if !root_meta.is_dir() {
        return Err("cacheInvalid".into());
    }

    let mut templates = Vec::new();
    let mut ids = HashSet::new();
    let mut paths_ci = HashSet::new();
    let entries = std::fs::read_dir(root).map_err(|_| "cacheInvalid".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "cacheInvalid".to_string())?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == AGENCY_MANIFEST_FILE {
            let meta = lstat_no_links(&entry.path()).map_err(|()| "cacheInvalid".to_string())?;
            if !meta.is_file() {
                return Err("cacheInvalid".into());
            }
            continue;
        }
        let division_meta =
            lstat_no_links(&entry.path()).map_err(|()| "cacheInvalid".to_string())?;
        if !division_meta.is_dir() {
            return Err("cacheInvalid".into());
        }
        let division_slug = slug_segment(&name).map_err(|_| "cacheInvalid".to_string())?;
        if division_slug != name {
            return Err("cacheInvalid".into());
        }
        for stem_entry in std::fs::read_dir(entry.path()).map_err(|_| "cacheInvalid".to_string())? {
            let stem_entry = stem_entry.map_err(|_| "cacheInvalid".to_string())?;
            let stem_name = stem_entry.file_name().to_string_lossy().to_string();
            let stem_meta =
                lstat_no_links(&stem_entry.path()).map_err(|()| "cacheInvalid".to_string())?;
            if !stem_meta.is_dir() {
                return Err("cacheInvalid".into());
            }
            let stem_slug = slug_segment(&stem_name).map_err(|_| "cacheInvalid".to_string())?;
            if stem_slug != stem_name {
                return Err("cacheInvalid".into());
            }
            let mut role_seen = false;
            for child in
                std::fs::read_dir(stem_entry.path()).map_err(|_| "cacheInvalid".to_string())?
            {
                let child = child.map_err(|_| "cacheInvalid".to_string())?;
                let child_name = child.file_name().to_string_lossy().to_string();
                let child_meta =
                    lstat_no_links(&child.path()).map_err(|()| "cacheInvalid".to_string())?;
                if child_name != "Role.md" || !child_meta.is_file() {
                    return Err("cacheInvalid".into());
                }
                role_seen = true;
            }
            if !role_seen {
                return Err("cacheInvalid".into());
            }
            let role_path = stem_entry.path().join("Role.md");
            let raw =
                std::fs::read_to_string(&role_path).map_err(|_| "cacheInvalid".to_string())?;
            let fm = parse_agency_template_frontmatter(&raw);
            let body = strip_yaml_frontmatter(&raw).trim().to_string();
            let id = format!("agency:{}-{}", division_slug, stem_slug);
            validate_role_template_body(&id, &body).map_err(|_| "cacheInvalid".to_string())?;
            if !ids.insert(id.clone()) {
                return Err("cacheInvalid".into());
            }
            let key = format!("{}\\{}", division_slug, stem_slug).to_ascii_lowercase();
            if !paths_ci.insert(key) {
                return Err("cacheInvalid".into());
            }
            templates.push(AgencyTemplate {
                id,
                name: fm.name.unwrap_or_else(|| title_case_slug(&stem_slug)),
                description: fm.description.unwrap_or_default(),
                category: title_case_slug(&division_slug),
                color: fm.color,
                body,
                role_path,
            });
        }
    }
    templates.sort_by(|a, b| {
        a.category
            .to_lowercase()
            .cmp(&b.category.to_lowercase())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(templates)
}

pub fn agency_template_metas_from_cache(config_dir: &Path) -> Vec<RoleTemplateMeta> {
    match load_cached_agency_templates(config_dir) {
        Ok(templates) => templates
            .into_iter()
            .map(|t| RoleTemplateMeta {
                id: t.id,
                source: "agency".into(),
                name: t.name,
                description: t.description,
                category: t.category,
                color: t.color,
                emoji: None,
                has_skills: false,
            })
            .collect(),
        Err(reason) if reason == "missing" => Vec::new(),
        Err(reason) => {
            log::error!(
                "[role-templates] Agency template cache is unavailable: {}",
                reason
            );
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Local enumeration + metadata collection
// ---------------------------------------------------------------------------

/// Enumerate valid local templates under `dir`. A valid template is a direct
/// child **real directory** (not a symlink/junction) containing a **real**
/// `Role.md` (not a symlink). Skips non-dirs, links/reparse points, dirs
/// without Role.md, and unreadable entries. Never recurses past depth 1.
///
/// A missing/unreadable `dir` yields `Vec::new()` — a missing templates dir
/// must not break the picker. Sorted case-insensitively by name on return.
fn collect_local_templates(dir: &Path) -> Vec<RoleTemplateMeta> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<RoleTemplateMeta> = Vec::new();
    for entry in entries.flatten() {
        // DirEntry::metadata() does NOT follow links — it stats the entry.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if is_link_or_reparse(&meta) || !meta.is_dir() {
            continue;
        }
        let path = entry.path();
        // Role.md must be a REAL file (not a symlink) for the template to count.
        let role_md = path.join("Role.md");
        match lstat_no_links(&role_md) {
            Ok(m) if m.is_file() => {}
            _ => continue,
        }
        let folder = entry.file_name().to_string_lossy().to_string();
        let id = format!("local:{}", folder);
        let raw = std::fs::read_to_string(&role_md).unwrap_or_default();
        let (name, description) = parse_template_frontmatter(&raw);
        let name = name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| folder.clone());
        // skills/ counts only if it is a REAL, non-empty directory.
        let skills = path.join("skills");
        let has_skills = matches!(lstat_no_links(&skills), Ok(m) if m.is_dir())
            && std::fs::read_dir(&skills)
                .map(|mut rd| rd.next().is_some())
                .unwrap_or(false);
        out.push(RoleTemplateMeta {
            id,
            source: "local".into(),
            name,
            description: description.unwrap_or_default(),
            category: "Local".into(),
            color: None,
            emoji: None,
            has_skills,
        });
    }
    // read_dir order is filesystem-arbitrary; §3.2 requires local templates
    // sorted by name (mirrors `list_all_agents`).
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Build the full metadata list: cached Agency + local (resolved path).
/// `config_dir` is a parameter for testability.
pub fn collect_role_templates(
    settings: &crate::config::settings::AppSettings,
    config_dir: &Path,
) -> Vec<RoleTemplateMeta> {
    let mut out: Vec<RoleTemplateMeta> = agency_template_metas_from_cache(config_dir);

    match resolve_local_templates_dir(settings, config_dir) {
        LocalTemplatesDir::Default(p) => {
            // ensure the default dir exists, then enumerate (empty is fine).
            let _ = ensure_default_templates_dir(config_dir);
            out.extend(collect_local_templates(&p));
        }
        LocalTemplatesDir::Custom(p) => out.extend(collect_local_templates(&p)),
        LocalTemplatesDir::InvalidCustom {
            configured,
            resolved,
        } => {
            // ErrorModal trigger (target starts with "agentscommander").
            log::error!(
                "[role-templates] Configured agentTemplatesPath \"{}\" is not a valid \
                 directory (resolved: {}). Cached Agency templates remain available. Fix or \
                 clear `agentTemplatesPath` in settings.json.",
                configured,
                resolved.display()
            );
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Template resolution (called by `create_agent_matrix`)
// ---------------------------------------------------------------------------

/// Resolve a template id to its body + skills source. Synchronous and
/// filesystem-read-only, so `create_agent_matrix` can call it BEFORE any disk
/// mutation. An unknown id is an `Err` — the caller `?`-propagates it and no
/// directory is created. `config_dir` is a parameter for testability; the
/// `agency:` branch reads only the validated cache under `config_dir`.
pub fn resolve_role_template(
    id: &str,
    settings: &crate::config::settings::AppSettings,
    config_dir: &Path,
) -> Result<ResolvedRoleTemplate, String> {
    if id.starts_with("agency:") {
        let t = load_cached_agency_templates(config_dir)?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| format!("Unknown Agency role template: {}", id))?;
        return Ok(ResolvedRoleTemplate {
            id: id.to_string(),
            body: t.body.trim().to_string(),
            skills_src: None,
        });
    }

    if id.starts_with("local:") {
        let dir = match resolve_local_templates_dir(settings, config_dir) {
            LocalTemplatesDir::Default(p) | LocalTemplatesDir::Custom(p) => p,
            LocalTemplatesDir::InvalidCustom {
                configured,
                resolved,
            } => {
                return Err(format!(
                    "Configured agentTemplatesPath \"{}\" is not a valid directory ({})",
                    configured,
                    resolved.display()
                ));
            }
        };
        // SECURITY: never build a path from the id string. Re-enumerate the
        // resolved dir and match by id — the resolved path is therefore always
        // a real direct child (no `..` / separator traversal possible).
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("Cannot read templates directory: {}", e))?;
        for entry in entries.flatten() {
            // The template folder must be a REAL directory. DirEntry::metadata()
            // does not follow links; `is_link_or_reparse` also rejects Windows
            // junctions. A symlinked entry is skipped.
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if is_link_or_reparse(&meta) || !meta.is_dir() {
                continue;
            }
            let folder = entry.file_name().to_string_lossy().to_string();
            if format!("local:{}", folder) != id {
                continue;
            }
            let path = entry.path();
            // Role.md must be a REAL file. A symlinked Role.md could exfiltrate
            // an arbitrary file's contents into the new agent's Role.md.
            let role_md = path.join("Role.md");
            let role_meta = lstat_no_links(&role_md).map_err(|()| {
                format!(
                    "Template \"{}\" Role.md is missing, unreadable, or a symlink",
                    folder
                )
            })?;
            if !role_meta.is_file() {
                return Err(format!(
                    "Template \"{}\" Role.md is not a regular file",
                    folder
                ));
            }
            let raw = std::fs::read_to_string(&role_md)
                .map_err(|e| format!("Template \"{}\" has no readable Role.md: {}", folder, e))?;
            let body = strip_yaml_frontmatter(&raw).trim().to_string();
            if body.is_empty() {
                return Err(format!("Template \"{}\" Role.md is empty", folder));
            }
            // skills/ is used only if it is a REAL directory. A symlinked or
            // junctioned skills/ is ignored: skills_src stays None rather than
            // copying a tree from outside the template folder.
            let skills = path.join("skills");
            let skills_src = match lstat_no_links(&skills) {
                Ok(m) if m.is_dir() => Some(skills),
                _ => None,
            };
            return Ok(ResolvedRoleTemplate {
                id: id.to_string(),
                body,
                skills_src,
            });
        }
        return Err(format!("Unknown local role template: {}", id));
    }

    Err(format!("Unrecognized role template id: {}", id))
}

// ---------------------------------------------------------------------------
// Recursive skills copy (best-effort, link-safe)
// ---------------------------------------------------------------------------

/// Recursively copy `src` into `dst`, best-effort and link-safe. Returns the
/// list of paths that could not be copied (empty ⇒ full success). Symlinks /
/// junctions / reparse points are skipped at the source root and at every
/// entry. Regular files and directories only.
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Vec<String> {
    let mut failures: Vec<String> = Vec::new();
    copy_dir_into(src, dst, &mut failures);
    failures
}

fn copy_dir_into(src: &Path, dst: &Path, failures: &mut Vec<String>) {
    // Reject a symlinked / reparse-point SOURCE before enumerating it.
    match std::fs::symlink_metadata(src) {
        Ok(m) if is_link_or_reparse(&m) => {
            failures.push(format!(
                "{}: source is a symlink/reparse point",
                src.display()
            ));
            return;
        }
        Ok(_) => {}
        Err(e) => {
            failures.push(format!("{}: cannot stat source ({})", src.display(), e));
            return;
        }
    }
    if let Err(e) = std::fs::create_dir_all(dst) {
        failures.push(format!("{}: cannot create dir ({})", dst.display(), e));
        return;
    }
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => {
            failures.push(format!("{}: cannot read dir ({})", src.display(), e));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                failures.push(format!("{}: bad dir entry ({})", src.display(), e));
                continue;
            }
        };
        // DirEntry::metadata() does NOT follow links — it stats the entry.
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                failures.push(format!("{}: cannot stat ({})", entry.path().display(), e));
                continue;
            }
        };
        if is_link_or_reparse(&meta) {
            continue; // symlink / junction — never copied (security).
        }
        let target = dst.join(entry.file_name());
        if meta.is_dir() {
            copy_dir_into(&entry.path(), &target, failures);
        } else if meta.is_file() {
            if let Err(e) = std::fs::copy(entry.path(), &target) {
                failures.push(format!("{}: cannot copy ({})", entry.path().display(), e));
            }
        }
        // else: device / fifo / other — skipped.
    }
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// List all role templates available to the New Agent picker: cached Agency
/// templates plus local templates from the resolved path.
///
/// Returns `Ok` even when the explicit `agentTemplatesPath` is invalid — that
/// failure is logged via `log::error!` (→ ErrorModal) and cached Agency
/// templates are still returned. A missing config dir returns an empty list.
#[tauri::command]
pub async fn list_role_templates(
    settings: tauri::State<'_, crate::config::settings::SettingsState>,
) -> Result<Vec<RoleTemplateMeta>, String> {
    let snapshot = settings.read().await.clone();
    match crate::config::config_dir() {
        Some(config_dir) => Ok(collect_role_templates(&snapshot, &config_dir)),
        None => {
            log::warn!(
                "[role-templates] could not resolve config dir; returning no role templates"
            );
            Ok(Vec::new())
        }
    }
}

#[tauri::command]
pub async fn get_agency_templates_status() -> Result<AgencyTemplatesStatus, String> {
    tauri::async_runtime::spawn_blocking(crate::cli::agency_templates::status_cache)
        .await
        .map_err(|e| format!("Agency template status task failed: {}", e))?
}

#[tauri::command]
pub async fn update_agency_templates() -> Result<crate::cli::agency_templates::UpdateResult, String>
{
    tauri::async_runtime::spawn_blocking(|| {
        crate::cli::agency_templates::update_cache(
            crate::cli::agency_templates::AgencyTemplatesUpdateArgs::default_ui_update(),
        )
    })
    .await
    .map_err(|e| format!("Agency template update task failed: {}", e))?
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AppSettings;
    use std::path::Path;

    // -- symlink test helpers (Windows needs SeCreateSymbolicLinkPrivilege) --

    #[cfg(unix)]
    fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }
    #[cfg(unix)]
    fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, contents).expect("write file");
    }

    fn seed_agency_cache(config: &Path, body: &str) {
        write(
            &agency_templates_dir(config)
                .join("testing")
                .join("accessibility-auditor")
                .join("Role.md"),
            body,
        );
        let manifest = AgencyTemplatesManifest {
            repo: "https://github.com/msitarzewski/agency-agents".into(),
            reference: "main".into(),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            template_count: 1,
        };
        write(
            &agency_manifest_path(config),
            &serde_json::to_string_pretty(&manifest).expect("manifest json"),
        );
    }

    #[test]
    fn missing_agency_cache_lists_no_agency_templates() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(agency_template_metas_from_cache(tmp.path()).is_empty());
        let status = agency_templates_status(tmp.path());
        assert!(!status.available);
        assert_eq!(status.reason.as_deref(), Some("missing"));
    }

    #[test]
    fn valid_agency_cache_lists_metadata() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_agency_cache(
            tmp.path(),
            "---\nname: Accessibility Auditor\ndescription: Finds a11y issues\ncolor: teal\n---\n\n# Body\n",
        );
        let list = agency_template_metas_from_cache(tmp.path());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "agency:testing-accessibility-auditor");
        assert_eq!(list[0].source, "agency");
        assert_eq!(list[0].name, "Accessibility Auditor");
        assert_eq!(list[0].description, "Finds a11y issues");
        assert_eq!(list[0].category, "Testing");
        assert_eq!(list[0].color.as_deref(), Some("teal"));
        assert!(!list[0].has_skills);
    }

    #[test]
    fn invalid_agency_manifest_returns_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_agency_cache(tmp.path(), "# Body\n");
        write(&agency_manifest_path(tmp.path()), "{not json");
        assert!(agency_template_metas_from_cache(tmp.path()).is_empty());
        let status = agency_templates_status(tmp.path());
        assert!(!status.available);
        assert_eq!(status.reason.as_deref(), Some("manifestMalformed"));
    }

    #[test]
    fn malformed_agency_commit_returns_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_agency_cache(tmp.path(), "# Body\n");
        let manifest = AgencyTemplatesManifest {
            repo: "https://github.com/msitarzewski/agency-agents".into(),
            reference: "main".into(),
            commit: "not-a-sha".into(),
            template_count: 1,
        };
        write(
            &agency_manifest_path(tmp.path()),
            &serde_json::to_string(&manifest).expect("manifest json"),
        );
        let status = agency_templates_status(tmp.path());
        assert!(!status.available);
        assert_eq!(status.reason.as_deref(), Some("invalidCommit"));
    }

    #[test]
    fn resolve_unknown_agency_id_errs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let r = resolve_role_template("agency:nope", &AppSettings::default(), tmp.path());
        assert!(r.is_err(), "unknown agency id must error");
    }

    #[test]
    fn resolve_cached_agency_id_ok() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_agency_cache(tmp.path(), "---\nname: A\n---\n\n# Cached Body\n");
        let r = resolve_role_template(
            "agency:testing-accessibility-auditor",
            &AppSettings::default(),
            tmp.path(),
        )
        .expect("known agency id must resolve");
        assert!(!r.body.trim().is_empty(), "resolved body must be non-empty");
        assert!(r.body.contains("Cached Body"));
        assert!(r.skills_src.is_none(), "agency templates ship no skills");
    }

    #[test]
    fn duplicate_agency_ids_reject_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &agency_templates_dir(tmp.path())
                .join("testing")
                .join("accessibility-auditor")
                .join("Role.md"),
            "# A\n",
        );
        write(
            &agency_templates_dir(tmp.path())
                .join("testing")
                .join("accessibility auditor")
                .join("Role.md"),
            "# B\n",
        );
        let manifest = AgencyTemplatesManifest {
            repo: "https://github.com/msitarzewski/agency-agents".into(),
            reference: "main".into(),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            template_count: 2,
        };
        write(
            &agency_manifest_path(tmp.path()),
            &serde_json::to_string(&manifest).expect("manifest json"),
        );
        assert!(load_cached_agency_templates(tmp.path()).is_err());
    }

    #[test]
    fn reserved_agency_body_delimiters_reject_validation() {
        let err =
            validate_role_template_body("agency:x", "# Body\n<!-- ac:role-profile\n").unwrap_err();
        assert!(err.contains("reserved"));
    }

    // -- 6: local resolution with/without skills --

    #[test]
    fn resolve_local_id_ok_with_and_without_skills() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("agent-templates");
        write(
            &base.join("tmpl-a").join("Role.md"),
            "# A\n\nRole A body.\n",
        );
        write(
            &base.join("tmpl-b").join("Role.md"),
            "# B\n\nRole B body.\n",
        );
        write(&base.join("tmpl-b").join("skills").join("x.md"), "skill\n");

        let s = AppSettings::default();
        let a = resolve_role_template("local:tmpl-a", &s, tmp.path()).expect("tmpl-a resolves");
        assert!(a.skills_src.is_none(), "tmpl-a has no skills/");
        let b = resolve_role_template("local:tmpl-b", &s, tmp.path()).expect("tmpl-b resolves");
        assert!(b.skills_src.is_some(), "tmpl-b ships skills/");
    }

    // -- 7: path traversal --

    #[test]
    fn resolve_local_id_traversal_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("agent-templates")).expect("mk dir");
        let r = resolve_role_template("local:../../evil", &AppSettings::default(), tmp.path());
        assert!(r.is_err(), "a traversal id must not resolve to any entry");
    }

    // -- 8-9: frontmatter stripping --

    #[test]
    fn resolve_local_id_strips_frontmatter() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path()
                .join("agent-templates")
                .join("fm")
                .join("Role.md"),
            "---\nname: FM\ndescription: D\n---\n\n# Body\n\nReal content.\n",
        );
        let r = resolve_role_template("local:fm", &AppSettings::default(), tmp.path())
            .expect("fm resolves");
        assert!(
            !r.body.starts_with("---"),
            "frontmatter must be stripped from the body: {:?}",
            r.body
        );
        assert!(r.body.contains("Real content."));
    }

    #[test]
    fn resolve_local_id_no_frontmatter_body_unchanged() {
        // A body that opens with a Markdown `---` horizontal rule but has NO
        // real frontmatter must keep that rule.
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path()
                .join("agent-templates")
                .join("hr")
                .join("Role.md"),
            "# Heading\n\n---\n\nText after a horizontal rule.\n",
        );
        let r = resolve_role_template("local:hr", &AppSettings::default(), tmp.path())
            .expect("hr resolves");
        assert!(
            r.body.contains("---"),
            "a markdown horizontal rule must not be mis-stripped: {:?}",
            r.body
        );
    }

    // -- 10-11: link safety --

    #[test]
    fn resolve_local_id_symlinked_role_md_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let secret = tmp.path().join("secret.txt");
        write(&secret, "TOP SECRET\n");
        let tmpl = tmp.path().join("agent-templates").join("evil");
        std::fs::create_dir_all(&tmpl).expect("mk template dir");
        // Symlink the template's Role.md at an outside file.
        let Ok(()) = try_symlink_file(&secret, &tmpl.join("Role.md")) else {
            return; // no symlink privilege — skip, not fail.
        };
        let r = resolve_role_template("local:evil", &AppSettings::default(), tmp.path());
        assert!(r.is_err(), "a symlinked Role.md must be rejected");
        if let Ok(t) = r {
            assert!(!t.body.contains("TOP SECRET"), "outside file leaked");
        }
    }

    #[test]
    fn resolve_local_id_symlinked_skills_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tmp.path().join("outside-skills");
        write(&outside.join("leak.md"), "leaked\n");
        let tmpl = tmp.path().join("agent-templates").join("ls");
        write(&tmpl.join("Role.md"), "# LS\n\nBody.\n");
        let Ok(()) = try_symlink_dir(&outside, &tmpl.join("skills")) else {
            return; // no symlink privilege — skip, not fail.
        };
        let r = resolve_role_template("local:ls", &AppSettings::default(), tmp.path())
            .expect("ls resolves (symlinked skills just ignored)");
        assert!(
            r.skills_src.is_none(),
            "a symlinked skills/ must be treated as absent"
        );
    }

    // -- 12: path resolution variants --

    #[test]
    fn resolve_local_templates_dir_variants() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = tmp.path();

        // unset ⇒ Default
        let unset = AppSettings::default();
        assert!(matches!(
            resolve_local_templates_dir(&unset, config),
            LocalTemplatesDir::Default(_)
        ));

        // relative override pointing at an existing dir ⇒ Custom(config/<rel>)
        std::fs::create_dir_all(config.join("rel-templates")).expect("mk rel dir");
        let rel = AppSettings {
            agent_templates_path: Some("rel-templates".into()),
            ..AppSettings::default()
        };
        match resolve_local_templates_dir(&rel, config) {
            LocalTemplatesDir::Custom(p) => assert_eq!(p, config.join("rel-templates")),
            other => panic!("expected Custom, got {:?}", other),
        }

        // absolute existing dir ⇒ Custom
        let abs_dir = tmp.path().join("abs-templates");
        std::fs::create_dir_all(&abs_dir).expect("mk abs dir");
        let abs = AppSettings {
            agent_templates_path: Some(abs_dir.to_string_lossy().to_string()),
            ..AppSettings::default()
        };
        assert!(matches!(
            resolve_local_templates_dir(&abs, config),
            LocalTemplatesDir::Custom(_)
        ));

        // non-existent override ⇒ InvalidCustom
        let bad = AppSettings {
            agent_templates_path: Some("does-not-exist-xyz".into()),
            ..AppSettings::default()
        };
        assert!(matches!(
            resolve_local_templates_dir(&bad, config),
            LocalTemplatesDir::InvalidCustom { .. }
        ));
    }

    // -- 13-14: default dir seeding --

    #[test]
    fn ensure_default_templates_dir_creates_dir_and_readme() {
        let tmp = tempfile::tempdir().expect("tempdir");
        ensure_default_templates_dir(tmp.path()).expect("seed default dir");
        assert!(tmp.path().join("agent-templates").is_dir());
        assert!(tmp
            .path()
            .join("agent-templates")
            .join("README.md")
            .is_file());
    }

    #[test]
    fn ensure_default_templates_dir_preserves_existing_readme() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let readme = tmp.path().join("agent-templates").join("README.md");
        write(&readme, "SENTINEL — user edited\n");
        ensure_default_templates_dir(tmp.path()).expect("seed default dir");
        let after = std::fs::read_to_string(&readme).expect("read readme");
        assert_eq!(
            after, "SENTINEL — user edited\n",
            "README must not be overwritten"
        );
    }

    // -- 15: combined collection --

    #[test]
    fn collect_role_templates_includes_cached_agency_and_local() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_agency_cache(tmp.path(), "# Agency\n\nBody.\n");
        write(
            &tmp.path()
                .join("agent-templates")
                .join("mine")
                .join("Role.md"),
            "# Mine\n\nLocal body.\n",
        );
        let all = collect_role_templates(&AppSettings::default(), tmp.path());
        assert!(
            all.iter().any(|t| t.source == "agency"),
            "seeded agency templates must be present"
        );
        let local = all
            .iter()
            .find(|t| t.id == "local:mine")
            .expect("the local template must be listed");
        assert_eq!(local.source, "local");
        assert_eq!(local.category, "Local");
    }

    // -- 16: local sort order --

    #[test]
    fn collect_local_templates_sorted_by_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("t");
        for folder in ["zeta", "alpha", "Mike"] {
            write(&base.join(folder).join("Role.md"), "# x\n\nbody\n");
        }
        let list = collect_local_templates(&base);
        let names: Vec<String> = list.iter().map(|t| t.name.to_lowercase()).collect();
        assert_eq!(names, vec!["alpha", "mike", "zeta"], "must be name-sorted");
    }

    // -- 17: BOM --

    #[test]
    fn parse_template_frontmatter_strips_bom() {
        let (name, desc) =
            parse_template_frontmatter("\u{FEFF}---\nname: Foo\ndescription: Bar\n---\n\nbody\n");
        assert_eq!(name.as_deref(), Some("Foo"));
        assert_eq!(desc.as_deref(), Some("Bar"));
    }

    // -- 18-20: recursive copy --

    #[test]
    fn copy_dir_recursive_copies_nested_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        write(&src.join("a.md"), "a\n");
        write(&src.join("nested").join("b.md"), "b\n");
        let dst = tmp.path().join("dst");
        let failures = copy_dir_recursive(&src, &dst);
        assert!(failures.is_empty(), "clean copy must report no failures");
        assert!(dst.join("a.md").is_file());
        assert!(dst.join("nested").join("b.md").is_file());
    }

    #[test]
    fn copy_dir_recursive_skips_symlink_entries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tmp.path().join("outside");
        write(&outside.join("leak.md"), "leak\n");
        let src = tmp.path().join("src");
        write(&src.join("real.md"), "real\n");
        let Ok(()) = try_symlink_dir(&outside, &src.join("linked")) else {
            return; // no symlink privilege — skip, not fail.
        };
        let dst = tmp.path().join("dst");
        let _ = copy_dir_recursive(&src, &dst);
        assert!(dst.join("real.md").is_file(), "real files copied");
        assert!(
            !dst.join("linked").exists(),
            "a symlink entry must never be copied"
        );
    }

    #[test]
    fn copy_dir_recursive_skips_symlink_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let real = tmp.path().join("real");
        write(&real.join("f.md"), "f\n");
        let link = tmp.path().join("link");
        let Ok(()) = try_symlink_dir(&real, &link) else {
            return; // no symlink privilege — skip, not fail.
        };
        let dst = tmp.path().join("dst");
        let failures = copy_dir_recursive(&link, &dst);
        assert!(
            !failures.is_empty(),
            "a symlinked source root must be rejected"
        );
        assert!(
            !dst.join("f.md").exists(),
            "nothing must be copied from a symlinked root"
        );
    }
}
