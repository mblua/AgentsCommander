export interface SessionRepo {
  label: string;
  sourcePath: string;
  branch: string | null;
}

export interface Session {
  id: string;
  name: string;
  shell: string;
  shellArgs: string[];
  effectiveShellArgs: string[] | null;
  createdAt: string;
  workingDirectory: string;
  status: SessionStatus;
  waitingForInput: boolean;
  pendingReview: boolean;
  lastPrompt: string | null;
  agentId: string | null;
  agentLabel: string | null;
  gitRepos: SessionRepo[];
  workgroupBrief: string | null;
  isCoordinator: boolean;
  isRootAgent: boolean;
  token: string;
  agentKind: CodingAgentKind | null;
}

export type SessionStatus = "active" | "running" | "idle" | { exited: number };

export type CodingAgentKind = "claude" | "codex" | "gemini";

export interface SessionGroup {
  id: string;
  name: string;
  color: string;
  collapsed: boolean;
  order: string[];
}

export interface ShellProfile {
  name: string;
  command: string;
  args: string[];
  icon: string;
  color: string;
  env: Record<string, string>;
  workingDirectory: string;
}

export interface AppConfig {
  general: GeneralConfig;
  sidebar: SidebarConfig;
  terminal: TerminalConfig;
  keybindings: Record<string, string>;
}

export interface GeneralConfig {
  defaultShell: string;
  defaultShellArgs: string[];
  theme: string;
  confirmOnClose: boolean;
}

export interface SidebarConfig {
  width: number;
  alwaysOnTop: boolean;
  opacity: number;
  showShellType: boolean;
  showStatusIcon: boolean;
}

export interface TerminalConfig {
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  scrollback: number;
  cursorStyle: "block" | "underline" | "bar";
  cursorBlink: boolean;
  webglRenderer: boolean;
}

export interface PtyOutputEvent {
  sessionId: string;
  data: number[];
}

export interface AgentConfig {
  id: string;
  label: string;
  command: string;
  color: string;
  gitPullBefore: boolean;
  excludeGlobalClaudeMd: boolean;
}

export interface RepoMatch {
  name: string;
  path: string;
  agents: string[];
}

export interface TelegramBotConfig {
  id: string;
  label: string;
  token: string;
  chatId: number;
  color: string;
}

export interface BridgeInfo {
  botId: string;
  botLabel: string;
  sessionId: string;
  status: BridgeStatus;
  color: string;
}

export type BridgeStatus = "active" | { error: string } | "detaching";

export interface WindowGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type MainSidebarSide = "left" | "right";

export interface AppSettings {
  defaultShell: string;
  defaultShellArgs: string[];
  agents: AgentConfig[];
  telegramBots: TelegramBotConfig[];
  restoreCoordinatorWakeState: boolean;
  sidebarAlwaysOnTop: boolean;
  raiseTerminalOnClick: boolean;
  soundsEnabled: boolean;
  teamIdleBeepEnabled: boolean;
  voiceToTextEnabled: boolean;
  geminiApiKey: string;
  geminiModel: string;
  voiceAutoExecute: boolean;
  voiceAutoExecuteDelay: number;
  sidebarZoom: number;
  terminalZoom: number;
  guideZoom: number;
  mainZoom: number;
  sidebarGeometry: WindowGeometry | null;
  terminalGeometry: WindowGeometry | null;
  mainGeometry: WindowGeometry | null;
  mainSidebarWidth: number;
  mainSidebarSide: MainSidebarSide;
  mainAlwaysOnTop: boolean;
  webServerEnabled: boolean;
  webServerPort: number;
  webServerBind: string;
  projectPath: string | null;
  projectPaths: string[];
  sidebarStyle: string;
  onboardingDismissed: boolean;
  coordSortByActivity: boolean;
  injectRtkHook: boolean;
  rtkPromptDismissed: boolean;
  autoGenerateBriefTitle: boolean;
  agentTemplatesPath: string | null;
  themeLight: boolean;
}

// Team grouping for sidebar
export interface TeamSessionGroup {
  team: Team;
  coordinator: Session | null;
  members: Session[];
}

// Team types (from discovery)

export interface TeamMember {
  name: string;
  path: string;
}

export interface Team {
  id: string;
  name: string;
  members: TeamMember[];
  coordinatorName?: string;
  layerId?: string;
  visible?: boolean;
}

// Sidebar store state
export interface SessionsState {
  sessions: Session[];
  activeId: string | null;
  teams: Team[];
  teamFilter: string | null;
  showInactive: boolean;
  showCategories: boolean;
  repos: RepoMatch[];
  coordSortByActivity: boolean;
  lastActivityBySessionId: Record<string, number>;
  hydrated: boolean;
}

// Phone communication types

export interface PhoneMessage {
  id: string;
  from: string;
  to: string;
  team: string;
  content: string;
  timestamp: string;
  status: "pending" | "delivered" | "error";
}

export interface PhoneConversation {
  id: string;
  participants: string[];
  createdAt: string;
  messages: PhoneMessage[];
}

export interface AgentInfo {
  name: string;
  path: string;
  teams: string[];
  isCoordinatorOf: string[];
}

// AC-new discovery types

export interface AcAgentMatrix {
  name: string;
  path: string;
  roleExists: boolean;
  preferredAgentId?: string;
}

export interface AcTeam {
  name: string;
  agents: string[];
  coordinator: string | null;
}

export interface AcAgentReplica {
  name: string;
  path: string;
  identityPath?: string;
  originProject?: string;
  preferredAgentId?: string;
  repoPaths: string[];
  repoBranch?: string;
  isCoordinator: boolean;
}

export interface AcWorkgroup {
  name: string;
  path: string;
  brief?: string;
  briefTitle?: string;
  agents: AcAgentReplica[];
  repoPath?: string;
  teamName?: string;
}

export interface AcDiscoveryResult {
  agents: AcAgentMatrix[];
  teams: AcTeam[];
  workgroups: AcWorkgroup[];
}

// Team wizard shared types (used by NewTeamModal and EditTeamModal)

export interface TeamWizardAgentEntry {
  name: string;
  path: string;
  projectName: string;
}

export interface TeamWizardRepoEntry {
  url: string;
  agents: Set<string>;
}

export type TeamWizardStep = 1 | 2 | 3;

export interface TeamConfigResult {
  agents: string[];
  coordinator: string;
  repos: { url: string; agents: string[] }[];
}

// ---------------------------------------------------------------------------
// Workgroup-delete blocker report (BLOCKERS: sentinel payload)
// Mirrors src-tauri/src/commands/wg_delete_diagnostic.rs structs.
// ---------------------------------------------------------------------------

export interface BlockerSession {
  sessionId: string;
  agentName: string;
  cwd: string;
}

export interface BlockerProcess {
  pid: number;
  name: string;
  cwd?: string;
  files: string[];
}

export interface BlockerReport {
  workgroup: string;
  platform: "windows" | "linux" | "macos" | "other";
  diagnosticAvailable: boolean;
  rawOsError: string;
  sessions: BlockerSession[];
  processes: BlockerProcess[];
}

// ---------------------------------------------------------------------------
// Brief mutation result (issue #162 — BRIEF action buttons)
// Mirrors src-tauri/src/commands/brief.rs::BriefUpdateResult.
// ---------------------------------------------------------------------------

export interface BriefUpdateResult {
  workgroupRoot: string;
  brief: string | null;
}

export interface WorkgroupBriefUpdatedEvent {
  workgroupRoot?: string;
  workgroupPath?: string;
  brief: string | null;
  sessionIds?: string[];
  briefTitle?: string;
}

// ---------------------------------------------------------------------------
// Project registration result (#191 — shared open/new project flow)
// Mirrors src-tauri/src/config/projects.rs::ProjectRegistration.
// ---------------------------------------------------------------------------

export interface ProjectRegistration {
  /** Absolute path that was added (or matched) in projectPaths. */
  path: string;
  /** True when this call appended a new entry, false when already present. */
  registered: boolean;
  /** True when this call created .ac-new/ on disk (always false for openProject). */
  created: boolean;
}

// ---------------------------------------------------------------------------
// Error-log modal (#264)
// Mirrors src-tauri/src/logging.rs::ErrorLogEntry.
// ---------------------------------------------------------------------------

export interface ErrorLogEntry {
  /** Local timestamp string, e.g. "2026-05-21 15:56:11.123". */
  timestamp: string;
  /** Always "ERROR" today — kept for forward-compat and copy output. */
  level: string;
  /** Log target, e.g. "agentscommander_lib::commands::entity_creation". */
  target: string;
  /** Full message; may contain newlines (multi-line git errors etc.). */
  message: string;
}

// ---------------------------------------------------------------------------
// Role template picker (#271)
// Mirrors src-tauri/src/commands/role_templates.rs::RoleTemplateMeta.
// ---------------------------------------------------------------------------

export interface RoleTemplateMeta {
  /** Source-qualified id: "agency:<stem>" or "local:<folder>". */
  id: string;
  /** "agency" | "local". */
  source: "agency" | "local";
  name: string;
  description: string;
  /** Display grouping label, e.g. "Engineering" or "Local". */
  category: string;
  color?: string | null;
  emoji?: string | null;
  hasSkills: boolean;
}

