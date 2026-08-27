import { isBrowser, isTauri } from "./platform";
import {
  executeUiTerminalController,
  measurePtyViewport,
  rememberSpawnViewport,
  resetUiTerminalControllerForTests,
  type UiTerminalOperation,
} from "./terminal-viewport";
import type {
  ListenOptions,
  Transport,
  TransportConnectionState,
  UnlistenFn,
} from "./transport";
import { TauriTransport } from "./transport-tauri";
import { WsTransport } from "./transport-ws";
import type {
  Session,
  SessionCommunication,
  SessionRepo,
  SessionContextPayload,
  SessionEnvWarningPayload,
  SessionWarning,
  PtyOutputEvent,
  PtyScreenSnapshot,
  AppSettings,
  SettingsSnapshot,
  LogLevel,
  UpdateInfo,
  AgentUpdateResult,
  AgentUpdateStatus,
  AgentUpdatePrompt,
  AgentUpdateCommandRef,
  AgentUpdateNode,
  AgentUpdateOverviewRow,
  AgentInstallStateChanged,
  CodingAgentEnv,
  CodingAgentDefinition,
  ReseedResult,
  CodingAgentProfilesConfig,
  RepoMatch,
  BridgeInfo,
  AcDiscoveryResult,
  ContextTemplateOverwriteResult,
  ContextTemplateUpdate,
  AcProjectRefreshRequestedPayload,
  ProjectGroupsUpdatedPayload,
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
  ArchivedProject,
  ProjectArchiveChanged,
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
  UiAutomationAction,
  UiAutomationRequest,
  UiAutomationResponse,
  ResourceKillRequest,
  ResourceKillResult,
  ResourceSnapshot,
  CoordinatorCloseOutcome,
  ScreenshotOverlayState,
  ScreenshotSelection,
  ScreenshotCaptureResult,
  ScreenshotCaptureFailedEvent,
  ScreenshotHotkeyStatus,
  WorkgroupGroupsConfig,
  NonStopReport,
  WebServerOwnedStatus,
  WebServerInterfaceInfo,
  ApiClientMintRequest,
  ApiClientMintResponse,
  SessionSelection,
  WatcherActivitySnapshot,
  WatcherAgentDraftEntry,
  WatcherDraftEntry,
  WatcherMatchBatch,
  WatcherPatternPreview,
  WatcherReachRow,
} from "./types";
import { decodeSessionSelection } from "./session-selection";

export type { UiTerminalOperation };

export interface SessionRepoInput {
  label: string;
  sourcePath: string;
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
  // `options` MUST be forwarded. A shim that declares fewer parameters than
  // `Transport.listen` still satisfies the type — TypeScript allows a narrower
  // implementation — so dropping it here is a silent, unchecked loss of the
  // event target, which is exactly what defeats the backend's `emit_to`.
  listen: <T>(
    event: string,
    callback: (payload: T) => void,
    options?: ListenOptions
  ) => currentTransport().listen<T>(event, callback, options),
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
  skipAutoResume?: boolean;
}

export interface RestartSessionOptions {
  agentId?: string;
  requestedProfile?: string | null;
  skipAutoResume?: boolean;
}

export interface CreateRootAgentOptions {
  agentId?: string;
  requestedProfile?: string | null;
}

/** @deprecated Issue #384 v1 compatibility: the legacy per-agent default/instance
 *  scope. Superseded by {@link ProfileAssignmentScope}. */
export type ProfileSelectionScope = "default" | "instance";

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
  create: async (opts?: CreateSessionOptions): Promise<Session> => {
    const viewport = isBrowser ? null : measurePtyViewport();

    const session = await transport.invoke<Session>("create_session", {
      shell: opts?.shell ?? null,
      shellArgs: opts?.shellArgs ?? null,
      cwd: opts?.cwd ?? null,
      sessionName: opts?.sessionName ?? null,
      agentId: opts?.agentId ?? null,
      requestedProfile: opts?.requestedProfile ?? null,
      gitRepos: opts?.gitRepos ?? null,
      skipAutoResume: opts?.skipAutoResume ?? null,
      cols: viewport?.cols ?? null,
      rows: viewport?.rows ?? null,
    });

    if (viewport) {
      rememberSpawnViewport(session.id, viewport);
    }

    return session;
  },

  destroy: (id: string) => transport.invoke<void>("destroy_session", { id }),

  closeCoordinator: (id: string, confirmed: boolean) =>
    transport.invoke<CoordinatorCloseOutcome>("close_coordinator", { id, confirmed }),

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

  getSelection: async (): Promise<SessionSelection> => {
    const value = await transport.invoke<unknown>("get_active_session");
    return decodeSessionSelection(value);
  },

  drainWarnings: (sessionId?: string | null) =>
    transport.invoke<SessionWarning[]>("drain_session_warnings", {
      sessionId: sessionId ?? null,
    }),

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
    if (transport.writePtyBinary) {
      transport.writePtyBinary(sessionId, data);
      return Promise.resolve();
    }
    return transport.invoke<void>("pty_write", {
      sessionId,
      data: Array.from(data),
    });
  },

  resize: (sessionId: string, cols: number, rows: number) =>
    transport.invoke<void>("pty_resize", { sessionId, cols, rows }),

  getScreenSnapshot: (sessionId: string) =>
    transport.invoke<PtyScreenSnapshot | null>("get_screen_snapshot", { sessionId }),

  getSessionContext: (sessionId: string) =>
    transport.invoke<number | null>("get_session_context", { sessionId }),

  /** #1171 - the session's watcher activity ring plus its loss and warm-up signals.
   *  A session with no buffer answers with an empty snapshot, never null and never an
   *  error, so the window's states are read from values and not from nullability. */
  getWatcherActivity: (sessionId: string, limit?: number) =>
    transport.invoke<WatcherActivitySnapshot>("get_watcher_activity", {
      sessionId,
      limit: limit ?? null,
    }),

  /** #1171 - compile a candidate pattern and, with a session, run it against its live rows.
   *  Omitting `sessionId` compiles only, which is the common case: writing a regex in
   *  Settings with no agent session running. A pattern that does not compile comes back as
   *  `compiles: false` with the message, not as a rejected call. */
  previewWatcherPattern: (pattern: string, sessionId?: string) =>
    transport.invoke<WatcherPatternPreview>("preview_watcher_pattern", {
      sessionId: sessionId ?? null,
      pattern,
    }),

  /**
   * #1171 - resolve the WHOLE Settings draft, watchers and agents, and answer per row which
   * agents its selector reaches and which of those it holds a slot on.
   *
   * Both halves travel because both are properties of the set, not of one row. Whether a
   * watcher is inside an agent's 8 slots depends on every other enabled row and on where
   * their ids fall in key order; and the modal edits agents and watchers in one store that
   * one Save writes together, so resolving against the saved agent list would answer about a
   * state the user has already left. A row-level call could only answer by inventing the rest
   * of the set, and with an empty saved map nine rows added before Save would all report that
   * they run while only eight do -- a positive claim about a watcher that will not run.
   *
   * Neither the stem rule nor the `BTreeMap` key order nor the number 8 is written a second
   * time here: they stay in Rust, and the frontend's `starts_with` rule must not be ported.
   *
   * `null` and an absent selector both mean "every agent"; `[]` means "none". The three stay
   * distinct all the way to Rust's `Option<Vec<String>>`.
   */
  previewWatcherReach: (
    watchers: WatcherDraftEntry[],
    agents: WatcherAgentDraftEntry[]
  ) =>
    transport.invoke<WatcherReachRow[]>("preview_watcher_reach", { watchers, agents }),

  subscribe: (sessionId: string) =>
    transport.invoke<{ rows: number; cols: number } | null>("subscribe_session", { sessionId }),

  getPtySize: (sessionId: string) =>
    transport.invoke<{ rows: number; cols: number }>("get_pty_size", { sessionId }),
};

export const CodingAgentsAPI = {
  getCatalog: () =>
    transport.invoke<CodingAgentDefinition[]>("get_coding_agent_catalog"),

  listReseedableCommands: () =>
    transport.invoke<string[]>("list_reseedable_agent_commands"),

  reseedDefault: (command: string) =>
    transport.invoke<ReseedResult>("reseed_coding_agent_default", { command }),
};

export const SettingsAPI = {
  // #1077: get_settings returns the flattened SettingsSnapshot (AppSettings +
  // projectPathResolution). update/save-draft still take a plain AppSettings;
  // the extra report field riding along on a round-tripped object is ignored by
  // the backend (non-deny_unknown_fields) and cannot re-pair persisted state.
  get: () => transport.invoke<SettingsSnapshot>("get_settings"),
  update: (settings: AppSettings) =>
    transport.invoke<void>("update_settings", { newSettings: settings }),
  saveDraft: (settings: AppSettings) =>
    transport.invoke<void>("save_settings_draft", { draft: settings }),
  openWebRemote: () => transport.invoke<void>("open_web_remote"),
  startWebServer: () => transport.invoke<boolean>("start_web_server"),
  stopWebServer: () => transport.invoke<boolean>("stop_web_server"),
  getWebServerStatus: () => transport.invoke<boolean>("get_web_server_status"),
  startApiServer: () => transport.invoke<boolean>("start_api_server"),
  stopApiServer: () => transport.invoke<boolean>("stop_api_server"),
  apiServerStatus: () => transport.invoke<boolean>("api_server_status"),
  mintApiClient: (request: ApiClientMintRequest) =>
    transport.invoke<ApiClientMintResponse>("mint_api_client", {
      root: request.root,
      scopes: request.scopes,
      label: request.label ?? null,
      expires: request.expires ?? null,
    }),
  getWebServerOwnedStatus: () =>
    transport.invoke<WebServerOwnedStatus>("get_web_server_owned_status"),
  listWebServerInterfaces: () =>
    transport.invoke<WebServerInterfaceInfo[]>("list_web_server_interfaces"),
  setSoundsEnabled: (value: boolean) =>
    transport.invoke<void>("set_sounds_enabled", { value }),
  setTerminalSnapshotsEnabled: (expected: boolean, enabled: boolean) =>
    transport.invoke<void>("set_terminal_snapshots_enabled", { expected, enabled }),
  setThemeLight: (value: boolean) =>
    transport.invoke<void>("set_theme_light", { value }),
  setMainResourceMonitorAttached: (value: boolean) =>
    transport.invoke<void>("set_main_resource_monitor_attached", { value }),
  setRailCollapse: (collapsedProjects: string[], favoritesCollapsed: boolean) =>
    transport.invoke<void>("set_rail_collapse", { collapsedProjects, favoritesCollapsed }),
  setLogLevel: (level: LogLevel) =>
    transport.invoke<void>("set_log_level", { level }),
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
  getUpdateStatus: () =>
    transport.invoke<UpdateInfo | null>("get_update_status"),
};

export const AgentUpdateAPI = {
  getStatus: () =>
    transport.invoke<AgentUpdateStatus | null>("get_agent_update_status"),
  answer: (command: string, enabled: boolean) =>
    transport.invoke<boolean>("agent_update_answer", { command, enabled }),
  /** #1551 - instant: the backend schedules the install probes in the background, only once the startup pass is finished. */
  getOverview: () =>
    transport.invoke<AgentUpdateOverviewRow[]>("get_agent_update_overview"),
};

export const ReposAPI = {
  search: (query: string) =>
    transport.invoke<RepoMatch[]>("search_repos", { query }),

  gitRemoteUrl: (path: string) =>
    transport.invoke<string | null>("git_remote_url", { path }),
};

/**
 * #1363 - this listener MUST be scoped to the window it runs in.
 *
 * The backend emits with `emit_to(label, ...)` once per attached window, but
 * Tauri short-circuits that label filter for any listener registered as
 * `EventTarget::Any` (`tauri-2.10.3/src/event/listener.rs:306-311`), and `Any`
 * is exactly what the JS `listen()` default sends
 * (`@tauri-apps/api/event.js:69-73`). Registering unscoped therefore delivers
 * every attached window's flush to every window that mounts a `TerminalView`,
 * and each pays the deserialization before dropping it — the bridge multiplier
 * of plan 7.4, which `emit_to` exists to remove, silently unfixed. No wrong
 * byte is ever written (the visibility filter at the single writer sees to
 * that); the cost is what regresses.
 *
 * The label itself is resolved by the transport, never passed from here.
 * Pinned by `TerminalView.attachment.test.tsx` and `transport-tauri.test.ts`.
 */
export function onPtyOutput(
  callback: (data: PtyOutputEvent) => void
): Promise<UnlistenFn> {
  return transport.listen<PtyOutputEvent>("pty_output", callback, {
    scopeToCurrentWindow: true,
  });
}

/**
 * #1363 - the two terminal-output attachment wrappers.
 *
 * Named `attachOutput` / `detachOutput` rather than `attach` / `detach`
 * because `WindowAPI.attach` / `detach` in this same file is a different
 * concept (moving a session into its own window).
 *
 * The window label is NOT an argument: Tauri takes it from the calling
 * webview, so a frontend can only ever attach the window it runs in and the
 * label can be neither forged nor misattributed.
 */
export const TerminalOutputAPI = {
  /** Attaches this window to the session's output and returns the seed
   *  snapshot, or `null` when there is nothing to seed from (an unavailable
   *  parser or a failed snapshot read still attaches, and the client then
   *  writes live with no reconcile). Rejects only when there is nothing to
   *  attach to: `sessionUnavailable` or `outputTargetUnavailable`.
   *
   *  `includeHistory` is always `true`: every seed is applied after a
   *  `terminal.reset()`, so replaying the 64 KiB ring cannot duplicate
   *  history, and dropping the seed on re-attach would instead hide
   *  everything the session produced while detached (plan 3.4.2). */
  attachOutput: (sessionId: string): Promise<PtyScreenSnapshot | null> =>
    transport.invoke<PtyScreenSnapshot | null>("activate_terminal_output", {
      sessionId,
      includeHistory: true,
    }),

  /** Releases this window's attachment. Never rejects for a session that is
   *  already gone: window close races session destroy. */
  detachOutput: (sessionId: string): Promise<void> =>
    transport.invoke<void>("detach_terminal_output", { sessionId }),
};

export function onSessionCreated(
  callback: (session: Session) => void
): Promise<UnlistenFn> {
  return transport.listen<Session>("session_created", callback);
}

export function onProjectArchiveChanged(
  callback: (event: ProjectArchiveChanged) => void
): Promise<UnlistenFn> {
  return transport.listen<ProjectArchiveChanged>("project_archive_changed", callback);
}

export function onSessionDestroyed(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string }>("session_destroyed", callback);
}

export function onSessionSwitched(
  callback: (data: SessionSelection, deliveryGeneration: number) => void
): Promise<UnlistenFn> {
  return transport.listen<unknown>("session_switched", (value) => {
    let selection: SessionSelection;
    try {
      selection = decodeSessionSelection(value);
    } catch (error) {
      console.error("[selection] Dropped malformed session_switched payload:", error);
      return;
    }
    const generation = currentTransport().connectionState().generation;
    callback(selection, generation);
  });
}

export function getTransportConnectionState(): TransportConnectionState {
  return currentTransport().connectionState();
}

export function onTransportConnectionState(
  callback: (state: TransportConnectionState) => void,
): Promise<UnlistenFn> {
  const subscribe = currentTransport().onConnectionState;
  return Promise.resolve(subscribe ? subscribe.call(currentTransport(), callback) : () => undefined);
}

export function isSelectionCoordinatorBusyError(error: unknown): boolean {
  return error === "selectionCoordinatorBusy";
}

export function onSessionRenamed(
  callback: (data: { id: string; name: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string; name: string }>(
    "session_renamed",
    callback
  );
}

export function onSessionCommunicationChanged(
  callback: (data: { sessionId: string; communication: SessionCommunication | null }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; communication: SessionCommunication | null }>(
    "session_communication_changed",
    callback
  );
}

export const VoiceAPI = {
  transcribe: (audio: number[], mimeType: string) =>
    transport.invoke<string>("voice_transcribe", { audio, mimeType }),
  markRecording: (sessionId: string, recording: boolean) =>
    transport.invoke<void>("voice_mark_recording", { sessionId, recording }),
  hadTyping: (sessionId: string) =>
    transport.invoke<boolean>("voice_had_typing", { sessionId }),
};

export const DebugAPI = {
  saveLogs: (content: string) =>
    transport.invoke<void>("save_debug_logs", { content }),
  drainErrorLogs: () =>
    transport.invoke<ErrorLogEntry[]>("drain_error_logs"),
};

export const AutomationAPI = {
  enabled: () => transport.invoke<boolean>("ui_automation_enabled"),
  frontendReady: (window: string) =>
    transport.invoke<void>("ui_automation_frontend_ready", { window }),
  complete: (result: UiAutomationResponse<UiAutomationAction>) =>
    transport.invoke<void>("ui_automation_complete", { result }),
  executeTerminalController: executeUiTerminalController,
  resetTerminalControllerForTests: resetUiTerminalControllerForTests,
};

export function onUiAutomationRequest(
  callback: (request: UiAutomationRequest<UiAutomationAction>) => void
): Promise<UnlistenFn> {
  return transport.listen<UiAutomationRequest<UiAutomationAction>>(
    "ui_automation_request",
    callback,
  );
}

export function onErrorLogEvent(
  callback: () => void
): Promise<UnlistenFn> {
  return transport.listen<unknown>("error_log_event", () => callback());
}

export const WindowAPI = {
  detach: (sessionId: string) =>
    transport.invoke<string>("detach_terminal", { sessionId }),

  attach: (sessionId: string) =>
    transport.invoke<void>("attach_terminal", { sessionId }),

  listDetached: () =>
    transport.invoke<string[]>("list_detached_sessions"),

  setDetachedGeometry: (sessionId: string, geometry: WindowGeometry) =>
    transport.invoke<void>("set_detached_geometry", { sessionId, geometry }),

  openInExplorer: (path: string) =>
    transport.invoke<void>("open_in_explorer", { path }),

  focusMain: () => transport.invoke<void>("focus_main_window"),

  /** @deprecated use focusMain(); back-compat alias, drop at v0.9 */
  ensureTerminal: () => transport.invoke<void>("focus_main_window"),

  openExternal: (url: string) =>
    transport.invoke<void>("open_external_url", { url }),

  openResourceMonitor: () =>
    transport.invoke<void>("open_resource_monitor_window"),

  dockResourceMonitor: () =>
    transport.invoke<void>("dock_resource_monitor_window"),

  /** #1171 - open the watcher activity window, or focus it and re-scope it to this session.
   *  Tauri-only: the web client has no such window and this command has no no-op arm there,
   *  so callers gate on `isTauri`. */
  openWatchers: (sessionId: string) =>
    transport.invoke<void>("open_watchers_window", { sessionId }),

  /**
   * #1171 - the durable half of the scope handover: what `open_watchers_window` last asked
   * for, whether or not a listener existed at the time.
   *
   * The window label exists the moment the builder returns, while the JavaScript listener
   * exists only after the bundle loads, Solid mounts and the subscription completes a round
   * trip. Tauri queues nothing for a listener that does not exist yet and the emit returns
   * `Ok` either way, so a second open during the load would drop the user's order in silence.
   * The window therefore subscribes first and then pulls this, exactly as it does for
   * matches. `null` means nothing has been requested yet.
   */
  getWatchersScope: () =>
    transport.invoke<string | null>("get_watchers_scope"),

  /** #1171 - persist the watcher window's geometry through a dedicated one-field command
   *  rather than `initWindowGeometry`, whose read-modify-write of the whole AppSettings
   *  races the Settings save that edits the watcher map. */
  setWatchersGeometry: (geometry: WindowGeometry) =>
    transport.invoke<void>("set_watchers_geometry", { geometry }),
};

export const ScreenshotAPI = {
  getOverlayState: (captureId: string, monitorId: number) =>
    transport.invoke<ScreenshotOverlayState>("screenshot_get_overlay_state", {
      captureId,
      monitorId,
    }),
  confirmSelection: (selection: ScreenshotSelection) =>
    transport.invoke<ScreenshotCaptureResult>("screenshot_confirm_selection", {
      selection,
    }),
  cancel: (captureId: string) =>
    transport.invoke<void>("screenshot_cancel_capture", { captureId }),
  getHotkeyStatus: () =>
    transport.invoke<ScreenshotHotkeyStatus>("screenshot_get_hotkey_status"),
  reloadHotkey: () =>
    transport.invoke<ScreenshotHotkeyStatus>("screenshot_reload_hotkey"),
};

export const ResourceMonitorAPI = {
  snapshot: () => transport.invoke<ResourceSnapshot>("get_resource_snapshot"),
  killGroup: (request: ResourceKillRequest) =>
    transport.invoke<ResourceKillResult>("kill_resource_group", { request }),
};

export const TaskAPI = {
  getTitle: (sessionId: string) =>
    transport.invoke<string | null>("task_get_title", { sessionId }),

  setTitle: (sessionId: string, title: string) =>
    transport.invoke<TaskUpdateResult>("task_set_title", { sessionId, title }),

  clean: (sessionId: string) =>
    transport.invoke<TaskUpdateResult>("task_clean", { sessionId }),

  cleanAt: (workgroupRoot: string) =>
    transport.invoke<TaskUpdateResult>("task_clean_at", { workgroupRoot }),

  setTitleAt: (workgroupRoot: string, title: string) =>
    transport.invoke<TaskUpdateResult>("task_set_title_at", { workgroupRoot, title }),
};

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

interface DiscoveryBranchUpdate {
  replicaPath: string;
  branch: string | null;
  repoBranches: (string | null)[];
  repoPaths: string[];
  repoDirty: (boolean | null)[];
}

export function onDiscoveryBranchUpdated(
  callback: (data: DiscoveryBranchUpdate) => void
): Promise<UnlistenFn> {
  return transport.listen<DiscoveryBranchUpdate>(
    "ac_discovery_branch_updated",
    callback
  );
}

export function onCoordinatorClockUpdated(
  callback: (data: { replicaPath: string; lastUserMessageAt: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ replicaPath: string; lastUserMessageAt: string }>(
    "coordinator_clock_updated",
    callback
  );
}

export function onCoordinatorAutoCloseChanged(
  callback: (data: { replicaPath: string; autoClosedAt: string | null }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ replicaPath: string; autoClosedAt: string | null }>(
    "coordinator_auto_close_changed",
    callback
  );
}

export function onCoordinatorManualCloseChanged(
  callback: (data: { replicaPath: string; manuallyClosedAt: string | null }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ replicaPath: string; manuallyClosedAt: string | null }>(
    "coordinator_manual_close_changed",
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

export function onProjectGroupsUpdated(
  callback: (data: ProjectGroupsUpdatedPayload) => void
): Promise<UnlistenFn> {
  return transport.listen<ProjectGroupsUpdatedPayload>(
    "project_groups_updated",
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

export function onSessionContext(
  callback: (data: SessionContextPayload) => void
): Promise<UnlistenFn> {
  return transport.listen<SessionContextPayload>("session_context", callback);
}

/**
 * #1171 - one tick's watcher matches for one session.
 *
 * Delivery is directed at the `watchers` window label and coalesced into one batch per
 * `(session, tick)`, and nothing is emitted at all while that window does not exist. The
 * ring still records, so opening the window later shows the history.
 */
export function onWatcherMatches(
  callback: (data: WatcherMatchBatch) => void
): Promise<UnlistenFn> {
  return transport.listen<WatcherMatchBatch>("watcher_matches", callback);
}

/**
 * #1171 - re-scope an already-open watcher window.
 *
 * The window is a singleton, so its `?sessionId=` query parameter is only read on first
 * creation; every later open focuses the existing window and arrives through here instead.
 */
export function onWatchersScopeRequest(
  callback: (data: { sessionId: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string }>("watchers_scope_request", callback);
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

export function onSessionEnvWarning(
  callback: (data: SessionEnvWarningPayload) => void
): Promise<UnlistenFn> {
  return transport.listen<SessionEnvWarningPayload>(
    "session_env_warning",
    callback
  );
}

export const AcDiscoveryAPI = {
  discover: () => transport.invoke<AcDiscoveryResult>("discover_ac_agents"),

  getReplicaContextFiles: (path: string) =>
    transport.invoke<string[]>("get_replica_context_files", { path }),

  setReplicaContextFiles: (path: string, files: string[]) =>
    transport.invoke<void>("set_replica_context_files", { path, files }),
};

export const ProjectAPI = {
  checkPath: (path: string) =>
    transport.invoke<boolean>("check_project_path", { path }),
  createAcProject: (path: string) =>
    transport.invoke<void>("create_ac_project", { path }),
  discover: (path: string) =>
    transport.invoke<AcDiscoveryResult>("discover_project", { path }),
  getGroups: (path: string) =>
    transport.invoke<WorkgroupGroupsConfig>("get_project_groups", { path }),
  updateGroups: (path: string, config: WorkgroupGroupsConfig) =>
    transport.invoke<WorkgroupGroupsConfig>("update_project_groups", { path, config }),
  keepCustomContextTemplate: (update: ContextTemplateUpdate) =>
    transport.invoke<void>("keep_custom_context_template", {
      path: update.projectPath,
      filename: update.filename,
      currentFileSha256: update.currentFileSha256,
      currentDefaultSha256: update.currentDefaultSha256,
    }),
  overwriteContextTemplateWithDefault: (update: ContextTemplateUpdate) =>
    transport.invoke<ContextTemplateOverwriteResult>(
      "overwrite_context_template_with_default",
      {
        path: update.projectPath,
        filename: update.filename,
        currentFileSha256: update.currentFileSha256,
        currentDefaultSha256: update.currentDefaultSha256,
      }
    ),
  open: (path: string) =>
    transport.invoke<ProjectRegistration>("open_project", { path }),
  new: (path: string) =>
    transport.invoke<ProjectRegistration>("new_project", { path }),
  remove: (path: string) => transport.invoke<void>("remove_project", { path }),
  archive: (path: string) => transport.invoke<void>("archive_project", { path }),
  unarchive: (path: string) =>
    transport.invoke<ProjectRegistration>("unarchive_project", { path }),
  listArchived: () =>
    transport.invoke<ArchivedProject[]>("list_archived_projects", {}),
};

export const NonStopAPI = {
  report: (reports: NonStopReport[]) =>
    transport.invoke<void>("non_stop_report", { reports }),
};

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

export const RoleTemplateAPI = {
  list: () => transport.invoke<RoleTemplateMeta[]>("list_role_templates"),
  status: () => transport.invoke<AgencyTemplatesStatus>("get_agency_templates_status"),
  updateAgencyTemplates: () =>
    transport.invoke<AgencyTemplatesUpdateResult>("update_agency_templates"),
};

function isUnknownRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function normalizeTeamConfigResult(value: unknown): TeamConfigResult {
  if (!isUnknownRecord(value)) {
    throw new Error("Invalid get_team_config response: expected an object");
  }

  const agents = value.agents;
  if (!isStringArray(agents)) {
    throw new Error(
      "Invalid get_team_config response: agents must be an array of strings",
    );
  }

  const coordinator = value.coordinator;
  if (typeof coordinator !== "string") {
    throw new Error("Invalid get_team_config response: coordinator must be a string");
  }

  const rawRepos = value.repos;
  if (!Array.isArray(rawRepos)) {
    throw new Error(
      "Invalid get_team_config response: repos must be an array of { url: string; agents: string[] }",
    );
  }
  const repos: { url: string; agents: string[] }[] = [];
  for (const rawRepo of rawRepos) {
    if (
      !isUnknownRecord(rawRepo)
      || typeof rawRepo.url !== "string"
      || !isStringArray(rawRepo.agents)
    ) {
      throw new Error(
        "Invalid get_team_config response: repos must be an array of { url: string; agents: string[] }",
      );
    }
    repos.push({ url: rawRepo.url, agents: [...rawRepo.agents] });
  }

  const rawContextAlertPercentages = value.contextAlertPercentages;
  let contextAlertPercentages: number[];
  if (rawContextAlertPercentages === undefined) {
    contextAlertPercentages = [];
  } else if (
    Array.isArray(rawContextAlertPercentages)
    && rawContextAlertPercentages.every(
      (percentage) => typeof percentage === "number" && Number.isFinite(percentage),
    )
  ) {
    contextAlertPercentages = [...rawContextAlertPercentages];
  } else {
    throw new Error(
      "Invalid get_team_config response: contextAlertPercentages must be an array of finite numbers",
    );
  }

  return {
    agents: [...agents],
    coordinator,
    repos,
    contextAlertPercentages,
  };
}

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

  deleteAgentMatrix: (projectPath: string, agentPath: string) =>
    transport.invoke<void>("delete_agent_matrix", { projectPath, agentPath }),

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
    repos: { url: string; agents: string[] }[],
    contextAlertPercentages: number[],
  ) =>
    transport.invoke<void>("create_team", {
      projectPath,
      name,
      agents,
      coordinator,
      repos,
      contextAlertPercentages,
    }),

  deleteTeam: (projectPath: string, teamName: string) =>
    transport.invoke<void>("delete_team", { projectPath, teamName }),

  updateTeam: (
    projectPath: string,
    teamName: string,
    agents: string[],
    coordinator: string,
    repos: { url: string; agents: string[] }[],
    contextAlertPercentages: number[],
  ) =>
    transport.invoke<void>("update_team", {
      projectPath,
      teamName,
      agents,
      coordinator,
      repos,
      contextAlertPercentages,
    }),

  getTeamConfig: (projectPath: string, teamName: string) =>
    transport.invoke<unknown>("get_team_config", { projectPath, teamName })
      .then(normalizeTeamConfigResult),

  createWorkgroup: (projectPath: string, teamName: string, taskTitle?: string) =>
    transport.invoke<void>("create_workgroup", {
      projectPath,
      teamName,
      taskTitle: taskTitle ?? null,
    }),

  deleteWorkgroup: (projectPath: string, workgroupName: string, force?: boolean) =>
    transport.invoke<void>("delete_workgroup", { projectPath, workgroupName, force: force ?? false }),
};

export const AgentCreatorAPI = {
  pickFolder: (defaultPath?: string) =>
    transport.invoke<string | null>("pick_folder", { defaultPath: defaultPath ?? null }),

  createFolder: (parentPath: string, agentName: string) =>
    transport.invoke<string>("create_agent_folder", { parentPath, agentName }),
};

export const GuideAPI = {
  open: () => transport.invoke<void>("open_guide_window"),
};

export const HomeAPI = {
  fetchMarkdown: () => transport.invoke<string>("fetch_home_markdown"),
};

export function emitThemeChanged(light: boolean): Promise<void> {
  return transport.emit("theme_changed", { light });
}

export function onThemeChanged(
  callback: (data: { light: boolean }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ light: boolean }>("theme_changed", callback);
}

export function emitResourceMonitorAttach(): Promise<void> {
  return transport.emit("resource_monitor_attach", {});
}

export function onResourceMonitorAttach(
  callback: () => void
): Promise<UnlistenFn> {
  return transport.listen<unknown>("resource_monitor_attach", () => callback());
}

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

export function onNpmUpdateAvailable(
  callback: (info: UpdateInfo) => void
): Promise<UnlistenFn> {
  return transport.listen<UpdateInfo>("npm_update_available", (info) =>
    callback(info)
  );
}

/** #1551 round 5 - the pass started; the payload carries its nodes in pass order (`null` from an older backend's unit payload). */
export function onAgentUpdatesStarted(
  callback: (payload: { nodes: AgentUpdateNode[] } | null) => void
): Promise<UnlistenFn> {
  return transport.listen<{ nodes: AgentUpdateNode[] } | null>(
    "agent_updates_started",
    (payload) => callback(payload ?? null)
  );
}

export function onAgentUpdatePrompt(
  callback: (prompt: AgentUpdatePrompt) => void
): Promise<UnlistenFn> {
  return transport.listen<AgentUpdatePrompt>("agent_update_prompt", (prompt) =>
    callback(prompt)
  );
}

/** A prompt stopped being pending (answered on any surface, or timed out after 60s).
 * #1551 - the payload names the closed prompt so a client clears only that one
 * (`null` when an older backend still sends its unit payload). */
export function onAgentUpdatePromptClosed(
  callback: (closed: AgentUpdateCommandRef | null) => void
): Promise<UnlistenFn> {
  return transport.listen<AgentUpdateCommandRef | null>(
    "agent_update_prompt_closed",
    (payload) => callback(payload ?? null)
  );
}

export function onAgentUpdatesFinished(
  callback: (payload: { results: AgentUpdateResult[] }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ results: AgentUpdateResult[] }>(
    "agent_updates_finished",
    (payload) => callback(payload)
  );
}

/** #1551 round 5 - a command's update sequence started; the payload is its pass node with `installBefore` filled. */
export function onAgentUpdateCommandStarted(
  callback: (node: AgentUpdateNode) => void
): Promise<UnlistenFn> {
  return transport.listen<AgentUpdateNode>("agent_update_command_started", (node) =>
    callback(node)
  );
}

/** #1551 round 5 - a prompted target left the pass (its prompt was answered No, or expired). */
export function onAgentUpdateCommandSkipped(
  callback: (ref: AgentUpdateCommandRef) => void
): Promise<UnlistenFn> {
  return transport.listen<AgentUpdateCommandRef>("agent_update_command_skipped", (ref) =>
    callback(ref)
  );
}

/** #1551 - a command's update sequence ended (ok, failed, or panicked). */
export function onAgentUpdateCommandFinished(
  callback: (result: AgentUpdateResult) => void
): Promise<UnlistenFn> {
  return transport.listen<AgentUpdateResult>("agent_update_command_finished", (result) =>
    callback(result)
  );
}

/** #1551 - a probe result was committed to the backend's install cache; carries that state's `seq`. */
export function onAgentInstallStateChanged(
  callback: (payload: AgentInstallStateChanged) => void
): Promise<UnlistenFn> {
  return transport.listen<AgentInstallStateChanged>(
    "agent_install_state_changed",
    (payload) => callback(payload)
  );
}

export function onCodingAgentProfilesUpdated(
  callback: () => void
): Promise<UnlistenFn> {
  return transport.listen<unknown>("coding_agent_profiles_updated", () => callback());
}

export function onCodingAgentSettingsUpdated(
  callback: (payload: { op: string; agentId: string | null }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ op: string; agentId: string | null }>(
    "coding_agent_settings_updated",
    (payload) => callback(payload)
  );
}

export function onScreenshotCaptureSaved(
  callback: (data: ScreenshotCaptureResult) => void
): Promise<UnlistenFn> {
  return transport.listen<ScreenshotCaptureResult>(
    "screenshot_capture_saved",
    callback
  );
}

export function onScreenshotCaptureFailed(
  callback: (data: ScreenshotCaptureFailedEvent) => void
): Promise<UnlistenFn> {
  return transport.listen<ScreenshotCaptureFailedEvent>(
    "screenshot_capture_failed",
    callback
  );
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
