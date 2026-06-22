import type { AgentConfig, AppSettings } from "../../shared/types";

/**
 * #529 (G3) — normalize the optional instructions filename to an honest stored
 * shape: trim a set value, and drop the field entirely when it is empty or
 * whitespace. The Config Screen input binds `value={agent.instructionsFilename
 * ?? ""}` and stores the raw string, so clearing it yields `Some("")`, which
 * Rust's `skip_serializing_if = "Option::is_none"` does NOT omit — it would
 * persist `"instructionsFilename": ""`, contradicting the "never persist a
 * sentinel" rule. Normalizing here, at the single save choke point, keeps the
 * per-keystroke `updateAgent` setter untouched and guarantees `""` never
 * reaches the backend. The backend stays tolerant either way (it trims an
 * empty value back to the command-derived default at launch).
 */
function normalizeAgentInstructionsFilename(agent: AgentConfig): AgentConfig {
  const trimmed = agent.instructionsFilename?.trim();
  if (trimmed) return { ...agent, instructionsFilename: trimmed };
  const { instructionsFilename: _drop, ...rest } = agent;
  return rest;
}

/**
 * #598 — normalize the optional config-folder seed to an honest stored shape.
 * The Coding Agents tab keeps `configSeed` as a live object while editing (so
 * the dest input and the enable toggle stay bound), but the backend uses
 * `#[serde(skip_serializing_if = "Option::is_none")]`: an inactive seed
 * (disabled, or an empty/whitespace `dest`) must serialize as omitted, not as
 * `{enabled:false}` or `{dest:""}`. Drop it unless it is genuinely active and
 * trim the kept `dest`, mirroring normalizeAgentInstructionsFilename. The
 * backend independently re-validates an active dest at save (rejecting
 * separators, `..`, `:`, etc.) and fails soft at launch, so this only governs
 * the persisted shape, never the validation.
 */
function normalizeAgentConfigSeed(agent: AgentConfig): AgentConfig {
  const dest = agent.configSeed?.dest?.trim();
  if (agent.configSeed?.enabled && dest) {
    return { ...agent, configSeed: { enabled: true, dest } };
  }
  const { configSeed: _drop, ...rest } = agent;
  return rest;
}

export function mergeSettingsForSavePreservingProjects(
  draft: AppSettings,
  fresh: AppSettings
): AppSettings {
  return {
    ...draft,
    agents: draft.agents
      .map(normalizeAgentInstructionsFilename)
      .map(normalizeAgentConfigSeed),
    projectPaths: fresh.projectPaths ?? [],
    projectPath: fresh.projectPath ?? null,
  };
}
