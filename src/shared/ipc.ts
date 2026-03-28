import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Session,
  PtyOutputEvent,
  AppSettings,
  RepoMatch,
  BridgeInfo,
  DarkFactoryConfig,
  PhoneMessage,
  AgentInfo,
} from "./types";

export interface CreateSessionOptions {
  shell?: string;
  shellArgs?: string[];
  cwd?: string;
  sessionName?: string;
  agentId?: string;
}

export const SessionAPI = {
  create: (opts?: CreateSessionOptions) =>
    invoke<Session>("create_session", {
      shell: opts?.shell ?? null,
      shellArgs: opts?.shellArgs ?? null,
      cwd: opts?.cwd ?? null,
      sessionName: opts?.sessionName ?? null,
      agentId: opts?.agentId ?? null,
    }),

  destroy: (id: string) => invoke<void>("destroy_session", { id }),

  switch: (id: string) => invoke<void>("switch_session", { id }),

  rename: (id: string, name: string) =>
    invoke<void>("rename_session", { id, name }),

  list: () => invoke<Session[]>("list_sessions"),

  getActive: () => invoke<string | null>("get_active_session"),

  setLastPrompt: (id: string, text: string) =>
    invoke<void>("set_last_prompt", { id, text }),
};

export const PtyAPI = {
  write: (sessionId: string, data: Uint8Array) =>
    invoke<void>("pty_write", { sessionId, data: Array.from(data) }),

  resize: (sessionId: string, cols: number, rows: number) =>
    invoke<void>("pty_resize", { sessionId, cols, rows }),
};

export const SettingsAPI = {
  get: () => invoke<AppSettings>("get_settings"),
  update: (settings: AppSettings) =>
    invoke<void>("update_settings", { newSettings: settings }),
};

export const ReposAPI = {
  search: (query: string) =>
    invoke<RepoMatch[]>("search_repos", { query }),
};

export function onPtyOutput(
  callback: (data: PtyOutputEvent) => void
): Promise<UnlistenFn> {
  return listen<PtyOutputEvent>("pty_output", (event) => {
    callback(event.payload);
  });
}

export function onSessionCreated(
  callback: (session: Session) => void
): Promise<UnlistenFn> {
  return listen<Session>("session_created", (e) => callback(e.payload));
}

export function onSessionDestroyed(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return listen<{ id: string }>("session_destroyed", (e) =>
    callback(e.payload)
  );
}

export function onSessionSwitched(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return listen<{ id: string }>("session_switched", (e) =>
    callback(e.payload)
  );
}

export function onSessionRenamed(
  callback: (data: { id: string; name: string }) => void
): Promise<UnlistenFn> {
  return listen<{ id: string; name: string }>("session_renamed", (e) =>
    callback(e.payload)
  );
}

// Voice API
export const VoiceAPI = {
  transcribe: (audio: number[], mimeType: string) =>
    invoke<string>("voice_transcribe", { audio, mimeType }),
  markRecording: (sessionId: string, recording: boolean) =>
    invoke<void>("voice_mark_recording", { sessionId, recording }),
  hadTyping: (sessionId: string) =>
    invoke<boolean>("voice_had_typing", { sessionId }),
};

// Debug API
export const DebugAPI = {
  saveLogs: (content: string) =>
    invoke<void>("save_debug_logs", { content }),
};

// Window API
export const WindowAPI = {
  detach: (sessionId: string) =>
    invoke<string>("detach_terminal", { sessionId }),

  closeDetached: (sessionId: string) =>
    invoke<void>("close_detached_terminal", { sessionId }),

  openInExplorer: (path: string) =>
    invoke<void>("open_in_explorer", { path }),

  ensureTerminal: () =>
    invoke<void>("ensure_terminal_window"),
};

// Telegram Bridge API
export const TelegramAPI = {
  attach: (sessionId: string, botId: string) =>
    invoke<BridgeInfo>("telegram_attach", { sessionId, botId }),

  detach: (sessionId: string) =>
    invoke<void>("telegram_detach", { sessionId }),

  listBridges: () => invoke<BridgeInfo[]>("telegram_list_bridges"),

  getBridge: (sessionId: string) =>
    invoke<BridgeInfo | null>("telegram_get_bridge", { sessionId }),

  sendTest: (token: string) =>
    invoke<number>("telegram_send_test", { token }),
};

export function onSessionGitBranch(
  callback: (data: { sessionId: string; branch: string | null }) => void
): Promise<UnlistenFn> {
  return listen<{ sessionId: string; branch: string | null }>(
    "session_git_branch",
    (e) => callback(e.payload)
  );
}

export function onSessionIdle(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return listen<{ id: string }>("session_idle", (e) => callback(e.payload));
}

export function onSessionBusy(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return listen<{ id: string }>("session_busy", (e) => callback(e.payload));
}

export function onTelegramBridgeAttached(
  callback: (data: BridgeInfo) => void
): Promise<UnlistenFn> {
  return listen<BridgeInfo>("telegram_bridge_attached", (e) =>
    callback(e.payload)
  );
}

export function onTelegramBridgeDetached(
  callback: (data: { sessionId: string }) => void
): Promise<UnlistenFn> {
  return listen<{ sessionId: string }>("telegram_bridge_detached", (e) =>
    callback(e.payload)
  );
}

export function onTelegramBridgeError(
  callback: (data: { sessionId: string; error: string }) => void
): Promise<UnlistenFn> {
  return listen<{ sessionId: string; error: string }>(
    "telegram_bridge_error",
    (e) => callback(e.payload)
  );
}

// Dark Factory API
export const DarkFactoryAPI = {
  get: () => invoke<DarkFactoryConfig>("get_dark_factory"),
  save: (config: DarkFactoryConfig) =>
    invoke<void>("save_dark_factory", { config }),
};

// Phone API
export const PhoneAPI = {
  sendMessage: (from: string, to: string, body: string, team: string) =>
    invoke<string>("phone_send_message", { from, to, body, team }),
  getInbox: (agentName: string) =>
    invoke<PhoneMessage[]>("phone_get_inbox", { agentName }),
  listAgents: () => invoke<AgentInfo[]>("phone_list_agents"),
  ackMessages: (agentName: string, messageIds: string[]) =>
    invoke<void>("phone_ack_messages", { agentName, messageIds }),
};

// Agent Creator API
export const AgentCreatorAPI = {
  pickFolder: (defaultPath?: string) =>
    invoke<string | null>("pick_folder", { defaultPath: defaultPath ?? null }),

  createFolder: (parentPath: string, agentName: string) =>
    invoke<string>("create_agent_folder", { parentPath, agentName }),
};

// Guide window
export const GuideAPI = {
  open: () => invoke<void>("open_guide_window"),
};

// Dark Factory window
export const DarkFactoryWindowAPI = {
  open: () => invoke<void>("open_darkfactory_window"),
};

export function onTelegramIncoming(
  callback: (data: { sessionId: string; text: string; from: string }) => void
): Promise<UnlistenFn> {
  return listen<{ sessionId: string; text: string; from: string }>(
    "telegram_incoming",
    (e) => callback(e.payload)
  );
}
