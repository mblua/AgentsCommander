// #2704 - static search index for Settings > General. One entry per user-visible
// control, in display order (category, then section, then field). Each `key` matches a
// `data-ac-setting` attribute on the control's wrapper in SettingsModal.tsx; the
// general-nav test fails if the two drift apart.

export type GeneralCategoryId = "appearance" | "terminal" | "agents" | "network" | "system";

export const GENERAL_CATEGORIES: { id: GeneralCategoryId; label: string }[] = [
  { id: "appearance", label: "Appearance" },
  { id: "terminal", label: "Terminal" },
  { id: "agents", label: "Agents" },
  { id: "network", label: "Network & remote access" },
  { id: "system", label: "System" },
];

export interface GeneralSettingEntry {
  key: string;
  category: GeneralCategoryId;
  section: string;
  label: string;
  keywords?: string;
}

export const GENERAL_SETTINGS_INDEX: GeneralSettingEntry[] = [
  // Appearance
  { key: "sidebarStyle", category: "appearance", section: "Window", label: "App Theme", keywords: "color colour dark light style skin" },
  { key: "selectedRowRailWidth", category: "appearance", section: "Window", label: "Selected Row Bar Width", keywords: "rail pixels px selection" },
  { key: "selectedRowRailColor", category: "appearance", section: "Window", label: "Selected Row Bar Color", keywords: "rail colour hex selection" },
  { key: "sidebarAlwaysOnTop", category: "appearance", section: "Window", label: "Sidebar always on top", keywords: "pin float above" },
  { key: "raiseTerminalOnClick", category: "appearance", section: "Window", label: "Raise terminal when clicking sidebar", keywords: "focus bring front" },
  { key: "roomNumberMask", category: "appearance", section: "Window", label: "Room number mask", keywords: "format digits padding" },
  { key: "screenshotCaptureHotkey", category: "appearance", section: "Hotkeys", label: "Screenshot hotkey", keywords: "shortcut keyboard capture" },
  { key: "sidebarCompactHotkey", category: "appearance", section: "Hotkeys", label: "Compact sidebar hotkey", keywords: "shortcut keyboard collapse" },
  { key: "soundsEnabled", category: "appearance", section: "Notifications", label: "Enable app sounds (master switch)", keywords: "audio mute volume" },
  { key: "teamIdleBeepEnabled", category: "appearance", section: "Notifications", label: "Beep when a team finishes working (all agents idle)", keywords: "sound audio alert done" },
  // Terminal
  { key: "defaultShell", category: "terminal", section: "Shell", label: "Default Shell (Complete path)", keywords: "bash zsh powershell cmd executable" },
  { key: "defaultShellArgs", category: "terminal", section: "Shell", label: "Shell Arguments", keywords: "args flags parameters" },
  { key: "typingHoldSeconds", category: "terminal", section: "Typing hold", label: "Hold message delivery after typing (seconds)", keywords: "delay wait keyboard" },
  { key: "responseCloseEnabled", category: "terminal", section: "Response close", label: "Close terminal after response", keywords: "coordinator request acknowledgement progress inactivity" },
  { key: "responseCloseIdleSeconds", category: "terminal", section: "Response close", label: "Idle seconds after response", keywords: "coordinator delay inactivity" },
  { key: "terminalSnapshotsEnabled", category: "terminal", section: "Terminal snapshots", label: "Allow authorized terminal snapshots", keywords: "capture screen json png" },
  // Agents
  { key: "restoreCoordinatorWakeState", category: "agents", section: "On app restart", label: "On start, wake orchestrators that were awake when the app closed", keywords: "resume restore startup" },
  { key: "restartResumeWakeWorkingAgents", category: "agents", section: "On app restart", label: "On start, wake agent replicas that were working when the app closed", keywords: "resume restore startup" },
  { key: "restartResumeOrchestratorPrompt", category: "agents", section: "On app restart", label: "Type into orchestrators that were working", keywords: "resume prompt text startup" },
  { key: "restartResumeAgentPrompt", category: "agents", section: "On app restart", label: "Type into agent replicas that were working", keywords: "resume prompt text startup" },
  { key: "coordinatorIdleBadgeYellowMinutes", category: "agents", section: "Orchestrator idle", label: "Badge turns yellow after (minutes)", keywords: "timer warning" },
  { key: "coordinatorIdleBadgeRedMinutes", category: "agents", section: "Orchestrator idle", label: "Badge turns red after (minutes)", keywords: "timer alert" },
  { key: "coordinatorAutoCloseEnabled", category: "agents", section: "Orchestrator idle", label: "Auto-close idle teams (terminate sessions to free resources)", keywords: "shutdown stop" },
  { key: "coordinatorAutoCloseMinutes", category: "agents", section: "Orchestrator idle", label: "Auto-close after (minutes of total silence)", keywords: "timeout shutdown" },
  { key: "coordinatorAutoCloseSkipTelegramAssigned", category: "agents", section: "Orchestrator idle", label: "Skip Telegram-assigned sessions during auto-close", keywords: "exclude bot" },
  { key: "coordinatorCascadeCloseEnabled", category: "agents", section: "Orchestrator idle", label: "Always close team members when manually closing Orchestrator", keywords: "cascade shutdown" },
  { key: "autoSelfClearEnabled", category: "agents", section: "Orchestrator idle", label: "Auto-clear and hand off context after 3 closed topics (on for orchestrators and Root; other agents opt in per agent)", keywords: "self clear handoff" },
  { key: "containerCredentialsFromHost", category: "agents", section: "Container Coding Agents", label: "Reuse host login for container coding agents", keywords: "credentials docker sign in token" },
  // Network & remote access
  { key: "apiServerEnabled", category: "network", section: "Control Plane API", label: "Enable API server", keywords: "http rest" },
  { key: "apiServerBind", category: "network", section: "Control Plane API", label: "IP/address", keywords: "bind host loopback interface" },
  { key: "apiServerPort", category: "network", section: "Control Plane API", label: "Port", keywords: "tcp" },
  { key: "apiClientMintRoot", category: "network", section: "Control Plane API", label: "Replica root", keywords: "api client credential mint" },
  { key: "apiClientMintScopes", category: "network", section: "Control Plane API", label: "Scopes", keywords: "api client credential permissions send list-peers-lean session-transport" },
  { key: "apiClientMintLabel", category: "network", section: "Control Plane API", label: "Label", keywords: "api client credential audit name" },
  { key: "apiClientMintExpiry", category: "network", section: "Control Plane API", label: "Expiry", keywords: "api client credential expires days hours" },
  { key: "webServerEnabled", category: "network", section: "Web Remote Access", label: "Enable web server", keywords: "browser phone mobile" },
  // System
  { key: "npmUpdateNotificationsEnabled", category: "system", section: "Updates", label: "Notify me when a new version is available", keywords: "upgrade release" },
  { key: "remoteBlockingMenusEnabled", category: "system", section: "Updates", label: "Download blocking-menu pattern updates from GitHub", keywords: "patterns sync" },
  { key: "logLevel", category: "system", section: "Logging", label: "Log level", keywords: "debug trace verbose verbosity" },
  { key: "activityLogEnabled", category: "system", section: "Logging", label: "Record activity log (activity.jsonl)", keywords: "diagnostic timeline" },
];

const CATEGORY_LABEL: Record<GeneralCategoryId, string> = Object.fromEntries(
  GENERAL_CATEGORIES.map((c) => [c.id, c.label]),
) as Record<GeneralCategoryId, string>;

export function generalCategoryLabel(id: GeneralCategoryId): string {
  return CATEGORY_LABEL[id];
}

/** Every whitespace token must be a substring of the entry text; results keep index order. */
export function searchGeneralSettings(query: string): GeneralSettingEntry[] {
  const q = query.trim().toLowerCase();
  if (q === "") return [];
  const tokens = q.split(/\s+/);
  return GENERAL_SETTINGS_INDEX.filter((entry) => {
    const haystack = `${CATEGORY_LABEL[entry.category]} ${entry.section} ${entry.label} ${entry.keywords ?? ""}`.toLowerCase();
    return tokens.every((t) => haystack.includes(t));
  });
}

export function countByCategory(results: GeneralSettingEntry[]): Record<GeneralCategoryId, number> {
  const counts = Object.fromEntries(GENERAL_CATEGORIES.map((c) => [c.id, 0])) as Record<
    GeneralCategoryId,
    number
  >;
  for (const r of results) counts[r.category] += 1;
  return counts;
}
