import type { AgentConfig, AppSettings } from "../../shared/types";

function normalizeAgentInstructionsFilename(agent: AgentConfig): AgentConfig {
  const trimmed = agent.instructionsFilename?.trim();
  if (trimmed) return { ...agent, instructionsFilename: trimmed };
  const { instructionsFilename: _drop, ...rest } = agent;
  return rest;
}

function normalizeAgentConfigSeed(agent: AgentConfig): AgentConfig {
  const dest = agent.configSeed?.dest?.trim();
  if (agent.configSeed?.enabled && dest) {
    return { ...agent, configSeed: { enabled: true, dest } };
  }
  const { configSeed: _drop, ...rest } = agent;
  return rest;
}

function normalizeAgentContextRegex(agent: AgentConfig): AgentConfig {
  if (agent.contextRegex && agent.contextRegex.trim()) return agent; // kept BYTE-FOR-BYTE
  const { contextRegex: _drop, ...rest } = agent;
  return rest;
}

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
      .map(normalizeAgentContextRegex)
      .map(normalizeAgentBackend),
    projectPaths: fresh.projectPaths ?? [],
    projectPath: fresh.projectPath ?? null,
    archivedProjectPaths: fresh.archivedProjectPaths ?? [],
  };
}
