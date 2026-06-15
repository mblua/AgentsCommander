import { isTauri } from "./platform";
import type { Transport, UnlistenFn } from "./transport";
import { TauriTransport } from "./transport-tauri";
import { WsTransport } from "./transport-ws";
import type {
  Session,
  SessionRepo,
  PtyOutputEvent,
  AppSettings,
  CodingAgentEnv,
  CodingAgentProfilesConfig,
  RepoMatch,
  BridgeInfo,
  PhoneMessage,
  AgentInfo,
  AcDiscoveryResult,
  AcProjectRefreshRequestedPayload,
  LoopConfigDetails,
  LoopCreateInput,
  LoopCronPreview,
  LoopEventPayload,
  LoopUpdateInput,
  TeamConfigResult,
  WindowGeometry,
  TaskUpdateResult,
  WorkgroupTaskUpdatedEvent,
  ProjectRegistration,
  ErrorLogEntry,
  AgencyTemplatesStatus,
  AgencyTemplatesUpdateResult,
  RoleTemplateMeta,
  CodingAgentProfileResolution,
  ProfileAssignmentScope,
  ProfileAssignmentError,
  PreviewCodingAgentProfileSelectionRequest,
  PreviewCodingAgentProfileSelectionResult,
  ApplyCodingAgentProfileSelectionRequest,
  ApplyCodingAgentProfileSelectionResult,
  SpecBoardDocument,
  SpecBoardSnapshot,
  SpecBoardChangedEvent,
  UiAutomationRequest,
  UiAutomationResponse
} from "./types";

export interface SessionRepoInput {
  label: string;
  sourcePath: string;
}

export type RtkStartupMode =
  | "prompt-enable"
  | "active"
  | "auto-disabled"
  | "silent";

export interface RtkSweepResult {
  total: number;
  succeeded: number;
  errors: { path: string; error: string }[];
}

function createDefaultTransport(): Transport {
  return isTauri ? new TauriTransport() : new WsTransport();
}

let defaultTransport: Transport | null = null;
let testTransport: Transport | null = null;

function currentTransport(): Transport {
  if (testTransport) return testTransport;
  defaultTransport ??= createDefaultTransport();
  return defaultTransport;
}

export function __setTransportForTests(next: Transport): () => void {
  if (import.meta.env.MODE !== "test") {
    throw new Error("__setTransportForTests is test-only");
  }

  const previous = testTransport;
  testTransport = next;
  return () => {
    testTransport = previous;
  };
}

const transport: Pick<Transport, "invoke" | "listen" | "emit"> = {
  invoke: <T>(cmd: string, args?: Record<string, unknown>) =>
    currentTransport().invoke<T>(cmd, args),
  listen: <T>(event: string, callback: (payload: T) => void) =>
    currentTransport().listen<T>(event, callback),
  emit: <T>(event: string, payload: T) =>
    currentTransport().emit<T>(event, payload),
};

export interface CreateSessionOptions {
  shell?: string;
  shellArgs?: string[];
  cwd?: string;
  sessionName?: string;
  agentId?: string;
  requestedProfile?: string | null;
  gitRepos?: SessionRepoInput[];
}

export interface RestartSessionOptions {
  agentId?: string;
  requestedProfile?: string | null;
  /**
   * Forwarded to the backend `restart_session` command. Omit (or pass `true`)
   * for a true user-intent restart that starts a fresh conversation. Pass
   * `false` when waking a deferred session — a session whose PTY is `Exited`
   * either because it was deferred at startup (new-policy default, or the
   * coord-was-asleep-at-shutdown branch of `restoreCoordinatorWakeState`) or
   * because the user closed it during the prior run — to allow provider
   * auto-resume (`claude --continue`, `codex resume --last`, `gemini --resume latest`).
   */
  skipAutoResume?: boolean;
}

export interface CreateRootAgentOptions {
  agentId?: string;
  requestedProfile?: string | null;
}

/** @deprecated Issue #384 v1 compatibility: the legacy per-agent default/instance
 *  scope. Superseded by {@link ProfileAssignmentScope}. */
export type ProfileSelectionScope = "default" | "instance";

/**
 * Single stable payload for `coding_agent_profile_selection_updated`, emitted by
 * both the legacy `set_agent_default_profile`/`set_instance_profile_override`
 * commands and the new broad-scope apply (#384 §7). All fields beyond `scope`
 * are optional because broad-scope applies touch many replicas (no single
 * `agentPath`). `scope` is a plain string to tolerate both the legacy
 * default/instance emitters and the v2 replica/kind/workgroup apply.
 */
export interface CodingAgentProfileSelectionUpdatedPayload {
  scope: ProfileAssignmentScope | ProfileSelectionScope | string;
  agentPath?: string;
  codingAgentId?: string;
  profile?: string | null;
  updatedCount?: number;
  restartedCount?: number;
  targetFingerprint?: string;
  errors?: ProfileAssignmentError[];
}

export const SessionAPI = {
  create: (opts?: CreateSessionOptions) =>
    transport.invoke<Session>("create_session", {
      shell: opts?.shell ?? null,
      shellArgs: opts?.shellArgs ?? null,
      cwd: opts?.cwd ?? null,
      sessionName: opts?.sessionName ?? null,
      agentId: opts?.agentId ?? null,
      requestedProfile: opts?.requestedProfile ?? null,
      gitRepos: opts?.gitRepos ?? null,
    }),

  destroy: (id: string) => transport.invoke<void>("destroy_session", { id }),

  restart: (id: string, opts?: RestartSessionOptions): Promise<Session> =>
    transport.invoke<Session>("restart_session", {
      id,
      agentId: opts?.agentId ?? null,
      requestedProfile: opts?.requestedProfile ?? null,
      skipAutoResume: opts?.skipAutoResume ?? null,
    }),

  switch: (id: string) => transport.invoke<void>("switch_session", { id }),

  rename: (id: string, name: string) =>
    transport.invoke<void>("rename_session", { id, name }),

  list: () => transport.invoke<Session[]>("list_sessions"),

  getActive: () => transport.invoke<string | null>("get_active_session"),

  setLastPrompt: (id: string, text: string) =>
    transport.invoke<void>("set_last_prompt", { id, text }),

  createRootAgent: (opts?: CreateRootAgentOptions) =>
    transport.invoke<Session>("create_root_agent_session", {
      agentId: opts?.agentId ?? null,
      requestedProfile: opts?.requestedProfile ?? null,
    }),
};

export const PtyAPI = {
  write: (sessionId: string, data: Uint8Array) => {
    const transport = currentTransport();
    // Use efficient binary transport if available (WS mode)
    if (transport.writePtyBinary) {
      transport.writePtyBinary(sessionId, data);
      return Promise.resolve();
    }
    // Fallback: JSON-encoded number array (Tauri mode)
    return transport.invoke<void>("pty_write", {
      sessionId,
      data: Array.from(data),
    });
  },

  resize: (sessionId: string, cols: number, rows: number) =>
    transport.invoke<void>("pty_resize", { sessionId, cols, rows }),

  /** Request screen snapshot replay for late-joining browser clients.
   *  Returns PTY dimensions so the browser can mirror them. */
  subscribe: (sessionId: string) =>
    transport.invoke<{ rows: number; cols: number } | null>("subscribe_session", { sessionId }),

  /** Get current PTY dimensions (rows, cols). */
  getPtySize: (sessionId: string) =>
    transport.invoke<{ rows: number; cols: number }>("get_pty_size", { sessionId }),
};

export const SettingsAPI = {
  get: () => transport.invoke<AppSettings>("get_settings"),
  update: (settings: AppSettings) =>
    transport.invoke<void>("update_settings", { newSettings: settings }),
  saveDraft: (settings: AppSettings) =>
    transport.invoke<void>("save_settings_draft", { draft: settings }),
  openWebRemote: () => transport.invoke<void>("open_web_remote"),
  startWebServer: () => transport.invoke<boolean>("start_web_server"),
  stopWebServer: () => transport.invoke<boolean>("stop_web_server"),
  getWebServerStatus: () => transport.invoke<boolean>("get_web_server_status"),
  // Narrow setters hold the SettingsState write lock through save_settings on
  // the Rust side, eliminating the IPC-level read-modify-write race that a
  // get+update round-trip would create against a concurrent update_settings
  // from SettingsModal. Used by RtkBanner.
  setInjectRtkHook: (value: boolean) =>
    transport.invoke<void>("set_inject_rtk_hook", { value }),
  setRtkPromptDismissed: (value: boolean) =>
    transport.invoke<void>("set_rtk_prompt_dismissed", { value }),
  setSoundsEnabled: (value: boolean) =>
    transport.invoke<void>("set_sounds_enabled", { value }),
  setThemeLight: (value: boolean) =>
    transport.invoke<void>("set_theme_light", { value }),
  updateCodingAgentProfiles: (profiles: CodingAgentProfilesConfig) =>
    transport.invoke<void>("update_coding_agent_profiles", { profiles }),
  updateCodingAgentEnvSettings: (
    agentId: string,
    envs: CodingAgentEnv[],
    isolatedHome: boolean,
  ) =>
    transport.invoke<void>("update_coding_agent_env_settings", {
      agentId,
      envs,
      isolatedHome,
    }),
  setAgentDefaultProfile: (agentPath: string, profile: string) =>
    transport.invoke<void>("set_agent_default_profile", { agentPath, profile }),
  setInstanceProfileOverride: (agentPath: string, profile: string | null) =>
    transport.invoke<void>("set_instance_profile_override", { agentPath, profile }),
  // #384 §7 — broad-scope assignment. Preview enumerates targets + live sessions
  // and returns a fingerprint; apply re-enumerates on the backend and rejects a
  // stale `confirmedTargetFingerprint`. The frontend target list is display-only;
  // the backend is authoritative for enumeration, writes, and restarts.
  previewCodingAgentProfileSelection: (
    request: PreviewCodingAgentProfileSelectionRequest,
  ) =>
    transport.invoke<PreviewCodingAgentProfileSelectionResult>(
      "preview_coding_agent_profile_selection",
      { request },
    ),
  applyCodingAgentProfileSelection: (
    request: ApplyCodingAgentProfileSelectionRequest,
  ) =>
    transport.invoke<ApplyCodingAgentProfileSelectionResult>(
      "apply_coding_agent_profile_selection",
      { request },
    ),
  resolveCodingAgentProfile: (
    agentPath: string | null,
    agentId: string,
    requestedProfile?: string | null,
  ) =>
    transport.invoke<CodingAgentProfileResolution>("resolve_coding_agent_profile", {
      agentPath,
      agentId,
      requestedProfile: requestedProfile ?? null,
    }),
  sweepRtkHook: (enabled: boolean) =>
    transport.invoke<RtkSweepResult>("sweep_rtk_hook", { enabled }),
  getRtkStartupStatus: () =>
    transport.invoke<RtkStartupMode>("get_rtk_startup_status"),
};

export const ReposAPI = {
  search: (query: string) =>
    transport.invoke<RepoMatch[]>("search_repos", { query }),
};

export function onPtyOutput(
  callback: (data: PtyOutputEvent) => void
): Promise<UnlistenFn> {
  return transport.listen<PtyOutputEvent>("pty_output", callback);
}

export function onSessionCreated(
  callback: (session: Session) => void
): Promise<UnlistenFn> {
  return transport.listen<Session>("session_created", callback);
}

export function onSessionDestroyed(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string }>("session_destroyed", callback);
}

export function onSessionSwitched(
  callback: (data: { id: string | null; userInitiated?: boolean }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string | null; userInitiated?: boolean }>(
    "session_switched",
    callback
  );
}

export function onSessionRenamed(
  callback: (data: { id: string; name: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string; name: string }>(
    "session_renamed",
    callback
  );
}

// Voice API
export const VoiceAPI = {
  transcribe: (audio: number[], mimeType: string) =>
    transport.invoke<string>("voice_transcribe", { audio, mimeType }),
  markRecording: (sessionId: string, recording: boolean) =>
    transport.invoke<void>("voice_mark_recording", { sessionId, recording }),
  hadTyping: (sessionId: string) =>
    transport.invoke<boolean>("voice_had_typing", { sessionId }),
};

// Debug API
export const DebugAPI = {
  saveLogs: (content: string) =>
    transport.invoke<void>("save_debug_logs", { content }),
  /** #264 — read-and-clear the backend's buffered ERROR-level log entries. */
  drainErrorLogs: () =>
    transport.invoke<ErrorLogEntry[]>("drain_error_logs"),
};

export const AutomationAPI = {
  enabled: () => transport.invoke<boolean>("ui_automation_enabled"),
  frontendReady: (window: string) =>
    transport.invoke<void>("ui_automation_frontend_ready", { window }),
  complete: (result: UiAutomationResponse) =>
    transport.invoke<void>("ui_automation_complete", { result }),
};

export function onUiAutomationRequest(
  callback: (request: UiAutomationRequest) => void
): Promise<UnlistenFn> {
  return transport.listen<UiAutomationRequest>("ui_automation_request", callback);
}

// #264 — content-free ping fired when a new ERROR-level log entry is captured.
// The listener responds by calling DebugAPI.drainErrorLogs().
export function onErrorLogEvent(
  callback: () => void
): Promise<UnlistenFn> {
  return transport.listen<unknown>("error_log_event", () => callback());
}

// Window API
export const WindowAPI = {
  detach: (sessionId: string) =>
    transport.invoke<string>("detach_terminal", { sessionId }),

  /**
   * Re-attach a detached session to the main window. Closes the detached
   * window, removes the session from DetachedSessionsState, switches main
   * to that session. Rust contract (plan §A2.2.G5): silent no-op if the
   * session was already destroyed.
   */
  attach: (sessionId: string) =>
    transport.invoke<void>("attach_terminal", { sessionId }),

  /**
   * Stateless authoritative list of detached session UUIDs. Used for
   * hydrating sessionsStore.detachedIds on SidebarApp mount (G.8 race
   * safety).
   */
  listDetached: () =>
    transport.invoke<string[]>("list_detached_sessions"),

  /**
   * Persist a detached window's geometry to its PersistedSession so it
   * re-spawns at the same position+size after an app restart. Per plan
   * §A2.4.Arb1 (R.6 option a) — backend stores the value on
   * PersistedSession.detached_geometry and auto-GCs when the session is
   * destroyed.
   */
  setDetachedGeometry: (sessionId: string, geometry: WindowGeometry) =>
    transport.invoke<void>("set_detached_geometry", { sessionId, geometry }),

  openInExplorer: (path: string) =>
    transport.invoke<void>("open_in_explorer", { path }),

  /**
   * Focus the main unified window (raising it, recreating if missing).
   * Rust command renamed from `ensure_terminal_window` → `focus_main_window`
   * in v0.8 (dev-rust owns that rename). Per plan §A2.4.Arb3 / R.4.
   */
  focusMain: () => transport.invoke<void>("focus_main_window"),

  /** @deprecated use focusMain(); back-compat alias, drop at v0.9 */
  ensureTerminal: () => transport.invoke<void>("focus_main_window"),

  // Open an http/https URL in the user's default browser. Backend rejects
  // non-http(s) schemes (issue #164).
  openExternal: (url: string) =>
    transport.invoke<void>("open_external_url", { url }),
};

// Brief API (issue #162)
export const TaskAPI = {
  getTitle: (sessionId: string) =>
    transport.invoke<string | null>("task_get_title", { sessionId }),

  setTitle: (sessionId: string, title: string) =>
    transport.invoke<TaskUpdateResult>("task_set_title", { sessionId, title }),

  clean: (sessionId: string) =>
    transport.invoke<TaskUpdateResult>("task_clean", { sessionId }),

};

// Telegram Bridge API
export const TelegramAPI = {
  attach: (sessionId: string, botId: string) =>
    transport.invoke<BridgeInfo>("telegram_attach", { sessionId, botId }),

  detach: (sessionId: string) =>
    transport.invoke<void>("telegram_detach", { sessionId }),

  listBridges: () => transport.invoke<BridgeInfo[]>("telegram_list_bridges"),

  getBridge: (sessionId: string) =>
    transport.invoke<BridgeInfo | null>("telegram_get_bridge", { sessionId }),

  sendTest: (token: string) =>
    transport.invoke<number>("telegram_send_test", { token }),

  // #282 — send an existing local file to the bot's configured chat.
  // ≤ 10 MB jpg/jpeg/png/webp ⇒ sendPhoto; otherwise sendDocument up to 50 MB.
  sendImage: (botId: string, path: string, caption?: string) =>
    transport.invoke<void>("telegram_send_image", { botId, path, caption }),
};

export function onPtyResized(
  callback: (data: { sessionId: string; rows: number; cols: number }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; rows: number; cols: number }>(
    "pty_resized",
    callback
  );
}

export function onTerminalDetached(
  callback: (data: { sessionId: string; windowLabel: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; windowLabel: string }>(
    "terminal_detached",
    callback
  );
}

export function onTerminalAttached(
  callback: (data: { sessionId: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string }>("terminal_attached", callback);
}

export function onSessionGitRepos(
  callback: (data: { sessionId: string; repos: SessionRepo[] }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; repos: SessionRepo[] }>(
    "session_git_repos",
    callback
  );
}

export function onSessionCoordinatorChanged(
  callback: (data: { sessionId: string; isCoordinator: boolean }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; isCoordinator: boolean }>(
    "session_coordinator_changed",
    callback
  );
}

export function onDiscoveryBranchUpdated(
  callback: (data: { replicaPath: string; branch: string | null }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ replicaPath: string; branch: string | null }>(
    "ac_discovery_branch_updated",
    callback
  );
}

export function onAcProjectRefreshRequested(
  callback: (data: AcProjectRefreshRequestedPayload) => void
): Promise<UnlistenFn> {
  return transport.listen<AcProjectRefreshRequestedPayload>(
    "ac_project_refresh_requested",
    callback
  );
}

export function onLoopEvent(
  callback: (data: LoopEventPayload) => void
): Promise<UnlistenFn> {
  return transport.listen<LoopEventPayload>("loop_event", callback);
}

export function onSessionIdle(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string }>("session_idle", callback);
}

export function onSessionBusy(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string }>("session_busy", callback);
}

export function onTelegramBridgeAttached(
  callback: (data: BridgeInfo) => void
): Promise<UnlistenFn> {
  return transport.listen<BridgeInfo>("telegram_bridge_attached", callback);
}

export function onTelegramBridgeDetached(
  callback: (data: { sessionId: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string }>(
    "telegram_bridge_detached",
    callback
  );
}

export function onTelegramBridgeError(
  callback: (data: { sessionId: string; error: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; error: string }>(
    "telegram_bridge_error",
    callback
  );
}

// Phone API
export const PhoneAPI = {
  sendMessage: (from: string, to: string, body: string, team: string) =>
    transport.invoke<string>("phone_send_message", { from, to, body, team }),
  getInbox: (agentName: string) =>
    transport.invoke<PhoneMessage[]>("phone_get_inbox", { agentName }),
  listAgents: () => transport.invoke<AgentInfo[]>("phone_list_agents"),
  ackMessages: (agentName: string, messageIds: string[]) =>
    transport.invoke<void>("phone_ack_messages", { agentName, messageIds }),
};

// AC Discovery API
export const AcDiscoveryAPI = {
  discover: () => transport.invoke<AcDiscoveryResult>("discover_ac_agents"),

  getReplicaContextFiles: (path: string) =>
    transport.invoke<string[]>("get_replica_context_files", { path }),

  setReplicaContextFiles: (path: string, files: string[]) =>
    transport.invoke<void>("set_replica_context_files", { path, files }),
};

// Project API
export const ProjectAPI = {
  checkPath: (path: string) =>
    transport.invoke<boolean>("check_project_path", { path }),
  createAcProject: (path: string) =>
    transport.invoke<void>("create_ac_project", { path }),
  discover: (path: string) =>
    transport.invoke<AcDiscoveryResult>("discover_project", { path }),
  /**
   * Validate an existing AC project at `path` and register it in
   * settings.projectPaths. Wraps the `open_project` Tauri command added in
   * #191 — same backend logic as the CLI `open-project` verb.
   */
  open: (path: string) =>
    transport.invoke<ProjectRegistration>("open_project", { path }),
  /**
   * Ensure an AC project at `path` (create `.ac/` Project AC Root if missing) and register
   * it in settings.projectPaths. Wraps the `new_project` Tauri command added
   * in #191 — same backend logic as the CLI `new-project` verb.
   */
  new: (path: string) =>
    transport.invoke<ProjectRegistration>("new_project", { path }),
};

// Project Loops API
export const LoopAPI = {
  create: (projectPath: string, input: LoopCreateInput) =>
    transport.invoke<LoopConfigDetails>("create_loop", {
      request: {
        projectPath,
        id: input.id ?? null,
        name: input.name,
        expr: input.expr,
        workgroup: input.workgroup,
        promptBody: input.promptBody,
        busyCoordinator: input.busyCoordinator ?? null,
        enabled: input.enabled ?? null,
      },
    }),

  update: (projectPath: string, id: string, input: LoopUpdateInput) =>
    transport.invoke<LoopConfigDetails>("update_loop", {
      request: {
        projectPath,
        id,
        name: input.name ?? null,
        expr: input.expr ?? null,
        workgroup: input.workgroup ?? null,
        promptBody: input.promptBody ?? null,
        busyCoordinator: input.busyCoordinator ?? null,
        enabled: input.enabled ?? null,
      },
    }),

  delete: (projectPath: string, id: string) =>
    transport.invoke<void>("delete_loop", { projectPath, id }),

  setEnabled: (projectPath: string, id: string, enabled: boolean) =>
    transport.invoke<LoopConfigDetails>("toggle_loop", {
      projectPath,
      id,
      enabled,
    }),

  runNow: (projectPath: string, id: string) =>
    transport.invoke<LoopConfigDetails>("run_loop_now", { projectPath, id }),

  getConfig: (projectPath: string, id: string) =>
    transport.invoke<LoopConfigDetails>("get_loop_config", { projectPath, id }),

  previewCron: (expr: string) =>
    transport.invoke<LoopCronPreview>("preview_loop_cron", { expr }),
};

// Role template picker (#271)
export const RoleTemplateAPI = {
  list: () => transport.invoke<RoleTemplateMeta[]>("list_role_templates"),
  status: () => transport.invoke<AgencyTemplatesStatus>("get_agency_templates_status"),
  updateAgencyTemplates: () =>
    transport.invoke<AgencyTemplatesUpdateResult>("update_agency_templates"),
};

// Entity Creation API (agents, teams, workgroups)
export const EntityAPI = {
  createAgentMatrix: (
    projectPath: string,
    name: string,
    description: string,
    roleTemplateId?: string | null,
  ) =>
    transport.invoke<void>("create_agent_matrix", {
      projectPath,
      name,
      description,
      roleTemplateId: roleTemplateId ?? null,
    }),

  deleteAgentMatrix: (projectPath: string, agentName: string) =>
    transport.invoke<void>("delete_agent_matrix", { projectPath, agentName }),

  listAllAgents: (projectPaths: string[]) =>
    transport.invoke<{ name: string; description: string; path: string; projectName: string }[]>(
      "list_all_agents",
      { projectPaths }
    ),

  createTeam: (
    projectPath: string,
    name: string,
    agents: string[],
    coordinator: string,
    repos: { url: string; agents: string[] }[]
  ) =>
    transport.invoke<void>("create_team", { projectPath, name, agents, coordinator, repos }),

  deleteTeam: (projectPath: string, teamName: string) =>
    transport.invoke<void>("delete_team", { projectPath, teamName }),

  updateTeam: (
    projectPath: string,
    teamName: string,
    agents: string[],
    coordinator: string,
    repos: { url: string; agents: string[] }[]
  ) =>
    transport.invoke<void>("update_team", { projectPath, teamName, agents, coordinator, repos }),

  getTeamConfig: (projectPath: string, teamName: string) =>
    transport.invoke<TeamConfigResult>("get_team_config", { projectPath, teamName }),

  createWorkgroup: (projectPath: string, teamName: string, taskTitle?: string) =>
    transport.invoke<void>("create_workgroup", {
      projectPath,
      teamName,
      taskTitle: taskTitle ?? null,
    }),

  deleteWorkgroup: (projectPath: string, workgroupName: string, force?: boolean) =>
    transport.invoke<void>("delete_workgroup", { projectPath, workgroupName, force: force ?? false }),

  syncWorkgroupRepos: (projectPath: string, teamName: string) =>
    transport.invoke<{ workgroupsUpdated: number; replicasUpdated: number; errors: { replica: string; error: string }[] }>(
      "sync_workgroup_repos", { projectPath, teamName }
    ),
};

// Agent Creator API
export const AgentCreatorAPI = {
  pickFolder: (defaultPath?: string) =>
    transport.invoke<string | null>("pick_folder", { defaultPath: defaultPath ?? null }),

  createFolder: (parentPath: string, agentName: string) =>
    transport.invoke<string>("create_agent_folder", { parentPath, agentName }),

  writeClaudeSettingsLocal: (agentPath: string) =>
    transport.invoke<void>("write_claude_settings_local", { agentPath }),
};

// Guide window
export const GuideAPI = {
  open: () => transport.invoke<void>("open_guide_window"),
};

// Home content (issue #164)
export const HomeAPI = {
  fetchMarkdown: () => transport.invoke<string>("fetch_home_markdown"),
};

// Theme sync across windows
export function emitThemeChanged(light: boolean): Promise<void> {
  return transport.emit("theme_changed", { light });
}

export function onThemeChanged(
  callback: (data: { light: boolean }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ light: boolean }>("theme_changed", callback);
}

// Open the Settings modal (handled by sidebar ActionBar). Emitted from any
// window — e.g. a disabled mic button asking the user to configure voice.
// `section` targets a specific tab in SettingsModal (e.g. "integrations").
// Omit to open on the default tab.
export function emitOpenSettings(section?: string): Promise<void> {
  return transport.emit<{ section?: string }>(
    "open_settings",
    section ? { section } : {}
  );
}

export function onOpenSettings(
  callback: (section?: string) => void
): Promise<UnlistenFn> {
  return transport.listen<{ section?: string }>("open_settings", (data) =>
    callback(data?.section)
  );
}

export function onRtkStartupStatus(
  callback: (mode: RtkStartupMode) => void
): Promise<UnlistenFn> {
  return transport.listen<{ mode: RtkStartupMode }>(
    "rtk_startup_status",
    (data) => callback(data.mode)
  );
}

export function onCodingAgentProfilesUpdated(
  callback: () => void
): Promise<UnlistenFn> {
  return transport.listen<unknown>("coding_agent_profiles_updated", () => callback());
}

export function onCodingAgentEnvSettingsUpdated(
  callback: (data: { agentId: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ agentId: string }>(
    "coding_agent_env_settings_updated",
    callback
  );
}

export function onCodingAgentProfileSelectionUpdated(
  callback: (data: CodingAgentProfileSelectionUpdatedPayload) => void
): Promise<UnlistenFn> {
  return transport.listen<CodingAgentProfileSelectionUpdatedPayload>(
    "coding_agent_profile_selection_updated",
    callback
  );
}

export function onLastPrompt(
  callback: (data: { sessionId: string; text: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; text: string }>(
    "last_prompt",
    callback
  );
}

export function onTelegramIncoming(
  callback: (data: { sessionId: string; text: string; from: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; text: string; from: string }>(
    "telegram_incoming",
    callback
  );
}

export function onWorkgroupTaskUpdated(
  callback: (data: WorkgroupTaskUpdatedEvent) => void
): Promise<UnlistenFn> {
  return transport.listen<WorkgroupTaskUpdatedEvent>(

    "workgroup_task_updated",
    callback
  );
}

export const SpecBoardAPI = {
  open: () => transport.invoke<void>("open_spec_board_window"),
  new: (repoRoot?: string | null) =>
    transport.invoke<SpecBoardDocument>("spec_board_new", { repoRoot: repoRoot ?? null }),
  pickOpen: (repoRoot?: string | null) =>
    transport.invoke<SpecBoardDocument | null>("spec_board_pick_open", { repoRoot: repoRoot ?? null }),
  openFile: (path: string) =>
    transport.invoke<SpecBoardDocument>("spec_board_open", { path }),
  save: (docId: string, content: string) =>
    transport.invoke<SpecBoardDocument>("spec_board_save", { docId, content }),
  pickSave: (docId: string, content: string, repoRoot?: string | null) =>
    transport.invoke<SpecBoardDocument | null>("spec_board_pick_save", { docId, content, repoRoot: repoRoot ?? null }),
  updateContent: (docId: string, content: string) =>
    transport.invoke<SpecBoardDocument>("spec_board_update_content", { docId, content }),
  listSnapshots: (docId: string) =>
    transport.invoke<SpecBoardSnapshot[]>("spec_board_list_snapshots", { docId }),
  checkoutSnapshot: (docId: string, snapshotId: string) =>
    transport.invoke<SpecBoardDocument>("spec_board_checkout_snapshot", { docId, snapshotId }),
  applyExternal: (docId: string) =>
    transport.invoke<SpecBoardDocument>("spec_board_apply_external", { docId }),
  keepMine: (docId: string) =>
    transport.invoke<SpecBoardDocument>("spec_board_keep_mine", { docId }),
  close: (docId: string) =>
    transport.invoke<void>("spec_board_close", { docId }),
};

export function onSpecBoardChanged(callback: (payload: SpecBoardChangedEvent) => void): Promise<UnlistenFn> {
  return transport.listen<SpecBoardChangedEvent>("spec_board_changed", callback);
}

export function onSpecBoardConflict(callback: (payload: SpecBoardDocument) => void): Promise<UnlistenFn> {
  return transport.listen<SpecBoardDocument>("spec_board_conflict", callback);
}

export function onSpecBoardFileMissing(callback: (payload: { docId: string; path: string }) => void): Promise<UnlistenFn> {
  return transport.listen<{ docId: string; path: string }>("spec_board_file_missing", callback);
}
