import { isTauri } from "./platform";
import type { Transport, UnlistenFn } from "./transport";
import { TauriTransport } from "./transport-tauri";
import { WsTransport } from "./transport-ws";
import type {
  Session,
  PtyOutputEvent,
  AppSettings,
  RepoMatch,
  BridgeInfo,
  DarkFactoryConfig,
  PhoneMessage,
  AgentInfo,
  AcDiscoveryResult,
} from "./types";

// Select transport based on runtime environment
const transport: Transport = isTauri ? new TauriTransport() : new WsTransport();

export interface CreateSessionOptions {
  shell?: string;
  shellArgs?: string[];
  cwd?: string;
  sessionName?: string;
  agentId?: string;
  gitBranchSource?: string;
  gitBranchPrefix?: string;
}

export const SessionAPI = {
  create: (opts?: CreateSessionOptions) =>
    transport.invoke<Session>("create_session", {
      shell: opts?.shell ?? null,
      shellArgs: opts?.shellArgs ?? null,
      cwd: opts?.cwd ?? null,
      sessionName: opts?.sessionName ?? null,
      agentId: opts?.agentId ?? null,
      gitBranchSource: opts?.gitBranchSource ?? null,
      gitBranchPrefix: opts?.gitBranchPrefix ?? null,
    }),

  destroy: (id: string) => transport.invoke<void>("destroy_session", { id }),

  switch: (id: string) => transport.invoke<void>("switch_session", { id }),

  rename: (id: string, name: string) =>
    transport.invoke<void>("rename_session", { id, name }),

  list: () => transport.invoke<Session[]>("list_sessions"),

  getActive: () => transport.invoke<string | null>("get_active_session"),

  setLastPrompt: (id: string, text: string) =>
    transport.invoke<void>("set_last_prompt", { id, text }),
};

export const PtyAPI = {
  write: (sessionId: string, data: Uint8Array) => {
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
  openWebRemote: () => transport.invoke<void>("open_web_remote"),
  startWebServer: () => transport.invoke<boolean>("start_web_server"),
  stopWebServer: () => transport.invoke<boolean>("stop_web_server"),
  getWebServerStatus: () => transport.invoke<boolean>("get_web_server_status"),
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
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string }>("session_switched", callback);
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
};

// Window API
export const WindowAPI = {
  detach: (sessionId: string) =>
    transport.invoke<string>("detach_terminal", { sessionId }),

  closeDetached: (sessionId: string) =>
    transport.invoke<void>("close_detached_terminal", { sessionId }),

  openInExplorer: (path: string) =>
    transport.invoke<void>("open_in_explorer", { path }),

  ensureTerminal: () => transport.invoke<void>("ensure_terminal_window"),
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
};

export function onPtyResized(
  callback: (data: { sessionId: string; rows: number; cols: number }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; rows: number; cols: number }>(
    "pty_resized",
    callback
  );
}

export function onSessionGitBranch(
  callback: (data: { sessionId: string; branch: string | null }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; branch: string | null }>(
    "session_git_branch",
    callback
  );
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

// Dark Factory API
export const DarkFactoryAPI = {
  get: () => transport.invoke<DarkFactoryConfig>("get_dark_factory"),
  save: (config: DarkFactoryConfig) =>
    transport.invoke<void>("save_dark_factory", { config }),
};

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
};

// Project API
export const ProjectAPI = {
  checkPath: (path: string) =>
    transport.invoke<boolean>("check_project_path", { path }),
  createAcProject: (path: string) =>
    transport.invoke<void>("create_ac_project", { path }),
  discover: (path: string) =>
    transport.invoke<AcDiscoveryResult>("discover_project", { path }),
};

// Agent Creator API
export const AgentCreatorAPI = {
  pickFolder: (defaultPath?: string) =>
    invoke<string | null>("pick_folder", { defaultPath: defaultPath ?? null }),

  createFolder: (parentPath: string, agentName: string) =>
    invoke<string>("create_agent_folder", { parentPath, agentName }),

  writeClaudeSettingsLocal: (agentPath: string) =>
    invoke<void>("write_claude_settings_local", { agentPath }),
};

// Guide window
export const GuideAPI = {
  open: () => transport.invoke<void>("open_guide_window"),
};

export function onLastPrompt(
  callback: (data: { sessionId: string; text: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; text: string }>(
    "last_prompt",
    callback
  );
}

// Dark Factory window
export const DarkFactoryWindowAPI = {
  open: () => invoke<void>("open_darkfactory_window"),
};

export function onTelegramIncoming(
  callback: (data: { sessionId: string; text: string; from: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; text: string; from: string }>(
    "telegram_incoming",
    callback
  );
}
