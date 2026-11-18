import type { AgentConfig, AppSettings } from "../../shared/types";
import type { TempRoomAgentChoice } from "../../shared/temp-rooms";

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
  if (agent.contextRegex?.trim()) return agent; // kept BYTE-FOR-BYTE
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

function webServerFields(draft: AppSettings, fresh: AppSettings, modalSeed: AppSettings | null) {
  return modalSeed
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
}

function apiServerFields(draft: AppSettings, fresh: AppSettings, modalSeed: AppSettings | null) {
  return modalSeed
    ? {
        apiServerPort: Object.is(draft.apiServerPort, modalSeed.apiServerPort)
          ? fresh.apiServerPort
          : draft.apiServerPort,
        apiServerBind: Object.is(draft.apiServerBind, modalSeed.apiServerBind)
          ? fresh.apiServerBind
          : draft.apiServerBind,
      }
    : {};
}

// #4783: keys only written outside the modal (verified: no modal edit); fresh disk wins.
const OUTSIDE_WRITER_KEYS = [
  "mainSidebarWidth",
  "mainSidebarSide",
  "sidebarZoom",
  "terminalZoom",
  "mainZoom",
  "sidebarGeometry",
  "terminalGeometry",
  "alwaysShowSelectedWorkgroup",
  "coordSortByActivity",
  "onboardingDismissed",
  "themeLight",
  "mainResourceMonitorAttached",
  "watchersGeometry",
  "autoSelfClearByAgent",
] as const satisfies readonly (keyof AppSettings)[];

type WithTempRoom = AppSettings & { tempRoomAgent?: TempRoomAgentChoice };

function outsideWriterFields(draft: AppSettings, fresh: AppSettings) {
  const { tempRoomAgent: _drop, ...rest } = draft as WithTempRoom;
  const choice = (fresh as WithTempRoom).tempRoomAgent;
  const picked = Object.fromEntries(OUTSIDE_WRITER_KEYS.map((key) => [key, fresh[key]]));
  return {
    rest,
    picked: { ...picked, ...(choice ? { tempRoomAgent: choice } : {}) } as Partial<WithTempRoom>,
  };
}

export function mergeSettingsForSavePreservingProjects(
  draft: AppSettings,
  fresh: AppSettings,
  modalSeed: AppSettings | null = null
): AppSettings {
  const outside = outsideWriterFields(draft, fresh);
  return {
    ...outside.rest,
    ...webServerFields(draft, fresh, modalSeed),
    ...apiServerFields(draft, fresh, modalSeed),
    ...outside.picked,
    responseCloseEnabled: modalSeed && (draft.responseCloseEnabled ?? true) === (modalSeed.responseCloseEnabled ?? true)
      ? (fresh.responseCloseEnabled ?? true)
      : (draft.responseCloseEnabled ?? true),
    responseCloseIdleSeconds: modalSeed && (draft.responseCloseIdleSeconds ?? 30) === (modalSeed.responseCloseIdleSeconds ?? 30)
      ? (fresh.responseCloseIdleSeconds ?? 30)
      : (draft.responseCloseIdleSeconds ?? 30),
    roomIdleConfirmationSeconds: modalSeed && (draft.roomIdleConfirmationSeconds ?? 15) === (modalSeed.roomIdleConfirmationSeconds ?? 15)
      ? (fresh.roomIdleConfirmationSeconds ?? 15)
      : (draft.roomIdleConfirmationSeconds ?? 15),
    agents: draft.agents
      .map(normalizeAgentInstructionsFilename)
      .map(normalizeAgentConfigSeed)
      .map(normalizeAgentContextRegex)
      .map((agent) => draft.containerOptionsEnabled === true ? normalizeAgentBackend(agent) : agent),
    projectPaths: fresh.projectPaths ?? [],
    projectPath: fresh.projectPath ?? null,
    archivedProjectPaths: fresh.archivedProjectPaths ?? [],
    terminalSnapshotsEnabled: fresh.terminalSnapshotsEnabled,
    containerOptionsEnabled: fresh.containerOptionsEnabled,
  };
}
