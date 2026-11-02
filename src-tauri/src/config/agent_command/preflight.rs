//! #3516 - preflight stages: the ordered 0-2 commands a launch runs before the
//! coding agent (Coding Agent level first, then the effective profile cell), and
//! the identity frame that adds the configured chain to `cell_identity`.
//! Pure resolution only; nothing here runs a stage.

use std::fmt::Write as _;
use std::path::Path;

use super::normalize_legacy_agent_command;
use crate::config::coding_agent_profiles::ProfileResolution;
use crate::config::placeholders::{
    ac_placeholder_error, expand_placeholders_in_args, placeholder_context_for_launch_root,
    reject_unexpanded_markers, value_contains_ac_placeholder, PlaceholderContext,
};
use crate::config::settings::{AgentConfig, ProfileCellConfig};

pub const DEFAULT_PREFLIGHT_TIMEOUT_SECONDS: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreflightStageSource {
    CodingAgent,
    Profile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightStage {
    pub source: PreflightStageSource,
    /// Effective profile letter, e.g. "A".
    pub effective_profile: String,
    /// "Coding Agent" or "Profile {effective}".
    pub label: String,
    /// Trimmed raw text as saved.
    pub command_text: String,
    /// First token after placeholder expansion.
    pub program: String,
    /// Remaining tokens after expansion.
    pub args: Vec<String>,
    /// The stage's own timeout field, or the 60 s default.
    pub timeout_seconds: u32,
}

/// Trimmed preflight text, or `None` when empty or absent (not configured).
pub fn configured_preflight(raw: Option<&str>) -> Option<&str> {
    raw.map(|text| text.trim_matches(|c: char| c.is_ascii_whitespace()))
        .filter(|text| !text.is_empty())
}

/// One configured level: source, raw trimmed text, own timeout field.
struct ConfiguredStage<'a> {
    source: PreflightStageSource,
    text: &'a str,
    timeout: Option<u32>,
}

/// Configured levels in run order: Coding Agent, then the effective cell.
fn configured_stages<'a>(
    agent: &'a AgentConfig,
    cell: &'a ProfileCellConfig,
) -> Vec<ConfiguredStage<'a>> {
    let levels = [
        (
            PreflightStageSource::CodingAgent,
            agent.preflight.as_deref(),
            agent.preflight_timeout_seconds,
        ),
        (
            PreflightStageSource::Profile,
            cell.preflight.as_deref(),
            cell.preflight_timeout_seconds,
        ),
    ];
    levels
        .into_iter()
        .filter_map(|(source, raw, timeout)| {
            configured_preflight(raw).map(|text| ConfiguredStage {
                source,
                text,
                timeout,
            })
        })
        .collect()
}

fn effective_timeout(timeout: Option<u32>) -> u32 {
    timeout.unwrap_or(DEFAULT_PREFLIGHT_TIMEOUT_SECONDS)
}

pub fn resolve_preflight_stages(
    agent: &AgentConfig,
    resolution: &ProfileResolution,
    launch_path: Option<&Path>,
    placeholder_context: Option<&PlaceholderContext>,
) -> Result<Vec<PreflightStage>, String> {
    configured_stages(agent, &resolution.cell)
        .into_iter()
        .map(|stage| resolve_stage(stage, resolution, launch_path, placeholder_context))
        .collect()
}

fn resolve_stage(
    stage: ConfiguredStage<'_>,
    resolution: &ProfileResolution,
    launch_path: Option<&Path>,
    placeholder_context: Option<&PlaceholderContext>,
) -> Result<PreflightStage, String> {
    let label = match stage.source {
        PreflightStageSource::CodingAgent => "Coding Agent".to_string(),
        PreflightStageSource::Profile => format!("Profile {}", resolution.effective_profile),
    };
    let timeout_seconds = effective_timeout(stage.timeout);
    if timeout_seconds == 0 {
        return Err(format!(
            "{label} preflight timeout must be at least 1 second"
        ));
    }
    let normalized = normalize_legacy_agent_command(stage.text)
        .map_err(|e| format!("Invalid {label} preflight command: {e}"))?;
    let mut tokens = Vec::with_capacity(normalized.shell_args.len() + 1);
    tokens.push(normalized.shell);
    tokens.extend(normalized.shell_args);
    let tokens = if tokens.iter().any(|t| value_contains_ac_placeholder(t)) {
        let built;
        let context = match placeholder_context {
            Some(context) => context,
            None => {
                let root = launch_path.ok_or_else(|| ac_placeholder_error().to_string())?;
                built = placeholder_context_for_launch_root(root)?;
                &built
            }
        };
        expand_placeholders_in_args(&tokens, context)?
    } else {
        let marker_context = format!("{label} preflight");
        for token in &tokens {
            reject_unexpanded_markers(token, &marker_context, false)?;
        }
        tokens
    };
    let Some((program, args)) = tokens.split_first() else {
        return Err(format!("Invalid {label} preflight command: empty"));
    };
    Ok(PreflightStage {
        source: stage.source,
        effective_profile: resolution.effective_profile.clone(),
        label,
        command_text: stage.text.to_string(),
        program: program.clone(),
        args: args.to_vec(),
        timeout_seconds,
    })
}

/// Identity frame over the RAW configured chain, appended to the cell preimage.
/// `None` when no level is configured, so empty-chain identity bytes stay legacy.
/// Tokens are length-prefixed (UTF-8 bytes) so the frame stays injective.
pub fn preflight_identity_frame(agent: &AgentConfig, cell: &ProfileCellConfig) -> Option<String> {
    let stages = configured_stages(agent, cell);
    if stages.is_empty() {
        return None;
    }
    let mut buf = String::from("pf2\u{0}");
    for stage in stages {
        let tag = match stage.source {
            PreflightStageSource::CodingAgent => "ca",
            PreflightStageSource::Profile => "profile",
        };
        let _ = write!(buf, "stage\u{0}{tag}\u{0}");
        match normalize_legacy_agent_command(stage.text) {
            Ok(normalized) => {
                let _ = write!(buf, "argc\u{0}{}\u{0}", normalized.shell_args.len() + 1);
                let tokens = std::iter::once(&normalized.shell).chain(&normalized.shell_args);
                for token in tokens {
                    let _ = write!(buf, "a\u{0}{}\u{0}{token}", token.len());
                }
            }
            Err(_) => {
                let _ = write!(buf, "raw\u{0}{}\u{0}{}", stage.text.len(), stage.text);
            }
        }
        let _ = write!(buf, "t\u{0}{}\u{0}", effective_timeout(stage.timeout));
    }
    Some(buf)
}
