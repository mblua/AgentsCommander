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
  workgroupTask: string | null;
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

export type TelegramPollFailureLogLevel = "debug" | "warn" | "error";

export type TelegramPollRecoveryLogLevel =
  | "debug"
  | "info"
  | "warn"
  | "error";

export interface TelegramNetworkPollErrorLogging {
  firstFailureLevel: TelegramPollFailureLogLevel;
  transientRepeatLevel: TelegramPollFailureLogLevel;
  sustainedLevel: TelegramPollFailureLogLevel;
  sustainedAfterSeconds: number;
  sustainedRepeatSeconds: number;
  recoveryLevel: TelegramPollRecoveryLogLevel;
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
  telegramNetworkPollErrorLogging: TelegramNetworkPollErrorLogging;
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
  alwaysShowSelectedWorkgroup?: boolean;
  injectRtkHook: boolean;
  rtkPromptDismissed: boolean;
  informWhenRtkInstalled: boolean;
  autoGenerateTaskTitle: boolean;
  agentTemplatesPath: string | null;
  themeLight: boolean;
  specBoardEnabled: boolean;
  resourceMonitorEnabled: boolean;
  maxConcurrentAgentProcesses: number;
  resourceWatchdogAction: ResourceWatchdogAction;
  agentGroupWarnPrivateBytes: number;
  agentGroupKillPrivateBytes: number;
  agentProcessKillPrivateBytes: number;
  resourceKeepLastSnapshot: boolean;
  resourceBackoffPolling: boolean;
}

export type ResourceWatchdogAction = "warn" | "killGroup";
export type ResourceOverallState =
  | "ok"
  | "warn"
  | "critical"
  | "enforcing"
  | "unknown";
export type ResourceNetworkState = "unknown" | "observed";
export type ResourceGroupState =
  | "starting"
  | "running"
  | "terminating"
  | "terminated"
  | "quarantined"
  | "failedCleanup"
  | "unknownOwnership";
export type ResourceKillReason =
  | "user"
  | "watchdog"
  | "sessionDestroy"
  | "appShutdown"
  | "spawnRollback";

export interface ResourceProcessIdentity {
  pid: number;
  creationTime100ns: number;
}

export interface ResourceProcessSnapshot {
  identity?: ResourceProcessIdentity | null;
  pid: number;
  creationTime100ns?: number;
  parentPid?: number | null;
  parentIdentity?: ResourceProcessIdentity | null;
  name?: string;
  exeName?: string;
  privateBytes?: number | null;
  workingSetBytes?: number | null;
  cpuPercent?: number | null;
  owned?: boolean;
  killAllowed: boolean;
  depth?: number;
}

export interface ResourceAgentGroupSnapshot {
  sessionId: string;
  name: string;
  rootPid?: number | null;
  rootIdentity?: ResourceProcessIdentity | null;
  state: ResourceGroupState;
  descendantsObserved: boolean;
  processCount: number;
  privateBytes?: number | null;
  workingSetBytes?: number | null;
  cpuPercent?: number | null;
  networkState: ResourceNetworkState;
  networkSummary: string;
  killAllowed?: boolean;
  processes: ResourceProcessSnapshot[];
  lastError?: string | null;
}

export interface ResourceSnapshot {
  capturedAt: string;
  overallState: ResourceOverallState;
  monitorEnabled: boolean;
  activeAgentGroups: number;
  maxConcurrentAgentGroups: number;
  appPrivateBytes?: number | null;
  appWorkingSetBytes?: number | null;
  networkState: ResourceNetworkState;
  networkSummary: string;
  groups: ResourceAgentGroupSnapshot[];
  warnings: string[];
}

export interface ResourceKillRequest {
  sessionId: string;
  reason: ResourceKillReason;
}

export interface ResourceKillResult {
  sessionId: string;
  state: ResourceGroupState;
  killedProcesses?: ResourceProcessIdentity[];
  killedPids?: number[];
  quarantined: boolean;
  message: string;
}

export type UiAutomationAction = "query" | "click" | "setValue";

export interface UiAutomationRequest {
  requestId: string;
  token: string;
  window: string;
  action: UiAutomationAction;
  selector: string;
  value?: string | null;
  expiresAtUnixMs?: number | null;
}

export interface UiAutomationTargetRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface UiAutomationTarget {
  testId: string;
  role: string | null;
  state: string | null;
  tag: string;
  text: string;
  visible: boolean;
  disabled: boolean;
  checked: boolean | null;
  selected: boolean | null;
  pressed: boolean | null;
  expanded: boolean | null;
  rect: UiAutomationTargetRect | null;
}

export interface UiAutomationDiagnostics {
  devicePixelRatio: number;
  viewport: { width: number; height: number };
  topmost?: UiAutomationTarget | null;
  expiresAtUnixMs?: number | null;
  nowUnixMs?: number;
}

export type UiAutomationResponse =
  | {
      ok: true;
      requestId: string;
      window: string;
      action: UiAutomationAction;
      selector: string;
      target: UiAutomationTarget;
      diagnostics?: UiAutomationDiagnostics;
    }
  | {
      ok: false;
      requestId: string;
      window: string;
      action: UiAutomationAction;
      selector: string;
      error:
        | "missing_selector"
        | "duplicate_selector"
        | "target_hidden"
        | "target_obscured"
        | "target_disabled"
        | "timeout"
        | "unsupported_action"
        | "value_not_supported"
        | "automation_bridge_exception";
      message: string;
      available?: UiAutomationTarget[];
      diagnostics?: UiAutomationDiagnostics;
    };

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
  alwaysShowSelectedWorkgroup: boolean;
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

// AC discovery types

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
  task: string | null;
  taskTitle?: string | null;
  agents: AcAgentReplica[];
  repoPath?: string;
  teamName?: string;
}

export interface AcDiscoveryResult {
  agents: AcAgentMatrix[];
  teams: AcTeam[];
  workgroups: AcWorkgroup[];
}

export type AcProjectRefreshReason =
  | "projectRegistered"
  | "createAgentMatrix"
  | "workgroupCreated"
  | "workgroupRemoved"
  | "teamMembershipChanged"
  | "teamMembershipRemoved"
  | string;

export interface AcProjectRefreshRequestedPayload {
  id: string;
  projectPath: string;
  changedPath?: string | null;
  changedName?: string | null;
  reason: AcProjectRefreshReason;
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

export interface IgnoredSessionRecord {
  sessionId: string;
  agentName: string;
  cwd: string;
  status: string;
  waitingForInput: boolean;
}

export interface DiagnosticError {
  message: string;
  code?: number;
  meaning?: string;
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
  restartManagerAvailable?: boolean;
  restartManagerError?: DiagnosticError;
  rawDeleteError?: string;
  liveSessions?: BlockerSession[];
  exitedSessionRecordsIgnored?: IgnoredSessionRecord[];
  externalProcesses?: BlockerProcess[];
}

// ---------------------------------------------------------------------------
// Task mutation result
// Mirrors src-tauri/src/commands/task.rs::TaskUpdateResult.
// ---------------------------------------------------------------------------

export interface TaskUpdateResult {
  workgroupRoot: string;
  task: string | null;
}

export type WorkgroupTaskUpdatedEvent =
  | {
      source: "manual";
      workgroupRoot: string;
      task: string | null;
      taskTitle: string | null;
    }
  | {
      source: "poll";
      workgroupRoot: string;
      task: string | null;
      taskTitle: string | null;
      sessionIds: string[];
    };

// ---------------------------------------------------------------------------
// Project registration result (#191 — shared open/new project flow)
// Mirrors src-tauri/src/config/projects.rs::ProjectRegistration.
// ---------------------------------------------------------------------------

export interface ProjectRegistration {
  /** Absolute path that was added (or matched) in projectPaths. */
  path: string;
  /** True when this call appended a new entry, false when already present. */
  registered: boolean;
  /** True when this call created .ac/ on disk (always false for openProject). */
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

export interface AgencyTemplatesStatus {
  available: boolean;
  path: string;
  repo?: string | null;
  ref?: string | null;
  commit?: string | null;
  templateCount?: number | null;
  reason?: string | null;
}

export interface AgencyTemplatesUpdateResult {
  repo: string;
  ref: string;
  commit: string;
  templateCount: number;
  path: string;
  updated: boolean;
}

export interface SpecBoardDocument {
  docId: string;
  repoRoot: string;
  path: string | null;
  fileKind: SpecBoardFileKind;
  content: string;
  diagramSource: string;
  dirty: boolean;
  conflict: SpecBoardConflict | null;
  versionIndex: number;
  versionCount: number;
  updatedAtMs: number;
}

export type SpecBoardFileKind = "mermaid" | "markdown";

export interface SpecBoardConflict {
  path: string;
  pendingExternalContent: string;
  pendingExternalDiagramSource: string;
  detectedAtMs: number;
}

export interface SpecBoardSnapshot {
  id: string;
  label: string;
  createdAtMs: number;
  source: SpecBoardSnapshotSource;
  content: string;
  diagramSource: string;
  hash: string;
}

export type SpecBoardSnapshotSource = "initial" | "open" | "edit" | "save" | "external" | "checkout";

export interface SpecBoardChangedEvent {
  docId: string;
  path: string | null;
  content: string;
  diagramSource: string;
  versionIndex: number;
  versionCount: number;
  external: boolean;
}


