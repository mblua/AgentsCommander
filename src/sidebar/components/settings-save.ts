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

export function mergeSettingsForSavePreservingProjects(
  draft: AppSettings,
  fresh: AppSettings
): AppSettings {
  return {
    ...draft,
    agents: draft.agents.map(normalizeAgentInstructionsFilename),
    projectPaths: fresh.projectPaths ?? [],
    projectPath: fresh.projectPath ?? null,
  };
}
