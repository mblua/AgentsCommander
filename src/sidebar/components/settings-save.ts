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

/**
 * #868 - normalize the optional runtime backend. The editor preserves a hidden
 * image in the live draft while toggling Container <-> Local, but Local must
 * persist exactly like the historical default: no `backend` object at all.
 */
function normalizeAgentBackend(agent: AgentConfig): AgentConfig {
  const kind = agent.backend?.kind ?? "localProcess";
  if (kind === "localProcess") {
    const { backend: _drop, ...rest } = agent;
    return rest;
  }

  const image = agent.backend?.image?.trim();
  return {
    ...agent,
    backend: image ? { kind, image } : { kind },
  };
}

export function mergeSettingsForSavePreservingProjects(
  draft: AppSettings,
  fresh: AppSettings,
  modalSeed: AppSettings | null = null
): AppSettings {
  const webServerFields = modalSeed
    ? {
        webServerEnabled: Object.is(draft.webServerEnabled, modalSeed.webServerEnabled)
          ? fresh.webServerEnabled
          : draft.webServerEnabled,
        webServerPort: Object.is(draft.webServerPort, modalSeed.webServerPort)
          ? fresh.webServerPort
          : draft.webServerPort,
        webServerBind: Object.is(draft.webServerBind, modalSeed.webServerBind)
          ? fresh.webServerBind
          : draft.webServerBind,
      }
    : {};
  const apiServerFields = modalSeed
    ? {
        apiServerPort: Object.is(draft.apiServerPort, modalSeed.apiServerPort)
          ? fresh.apiServerPort
          : draft.apiServerPort,
        apiServerBind: Object.is(draft.apiServerBind, modalSeed.apiServerBind)
          ? fresh.apiServerBind
          : draft.apiServerBind,
      }
    : {};

  return {
    ...draft,
    ...webServerFields,
    ...apiServerFields,
    agents: draft.agents
      .map(normalizeAgentInstructionsFilename)
      .map(normalizeAgentConfigSeed)
      .map(normalizeAgentBackend),
    projectPaths: fresh.projectPaths ?? [],
    projectPath: fresh.projectPath ?? null,
    archivedProjectPaths: fresh.archivedProjectPaths ?? [],
  };
}
