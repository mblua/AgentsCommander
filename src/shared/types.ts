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
  requestedProfile: string | null;
  effectiveProfile: string | null;
  profileFallbackChain: string[];
  profileFallbackApplied: boolean;
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
  envs: CodingAgentEnv[];
  /**
   * v2 (#384): renamed from `isolateCodexHome`. Until adapters support other
   * providers, only Codex consumes this flag. Serialized as `isolatedHome`.
   */
  isolatedHome: boolean;
  /**
   * #529 — instructions filename written to the agent root at launch (content =
   * AC context + Role.md). Empty/undefined falls back to the command-derived
   * default (Claude → CLAUDE.md, Gemini → GEMINI.md, else AGENTS.md). The empty
   * string is normalized to omitted before save, so it is never persisted.
   */
  instructionsFilename?: string;
}

/**
 * `"system"` is the v2 name for AgentsCommander-managed rows. The legacy v1
 * value `"agentsCommander"` is migrated to `"system"` by the backend on load,
 * so the frontend only ever sees v2 values after `SettingsAPI.get()`.
 */
export type CodingAgentEnvSource = "user" | "system";

export interface CodingAgentEnv {
  key: string;
  value: string;
  source: CodingAgentEnvSource;
  enabled: boolean;
}

export interface CodingAgentProfilesConfig {
  schemaVersion: number;
  /** v2 (#384): renamed from `letters`. Keyed by profile slot letter A–Z. */
  profileSlots: Record<string, ProfileSlotConfig>;
  /** v2 (#384): renamed from `agentDefaults`. agentId → default profile letter. */
  defaultProfileByAgent: Record<string, string>;
  /** v2 (#384): renamed from `matrix`. agentId → letter → cell. */
  profilesByAgent: Record<string, Record<string, ProfileCellConfig>>;
}

export interface ProfileSlotConfig {
  /** v2 (#384): renamed from `name`. Human display label of the profile slot. */
  label: string;
}

export interface ProfileCellConfig {
  enabled: boolean;
  /**
   * v2 (#384): one complete invocation string per profile, replacing the v1
   * `argv` array. An empty string falls back to `agents[].command` at launch.
   */
  command: string;
  env: Record<string, string>;
  notes: string;
}

export interface CodingAgentProfileResolution {
  requestedProfile: string;
  effectiveProfile: string;
  fallbackChain: string[];
  fallbackApplied: boolean;
  requestedProfileInput: string | null;
  instanceProfileOverride: string | null;
  originDefaultProfile: string | null;
  agentDefaultProfile: string | null;
  warnings: string[];
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
  codingAgentProfiles: CodingAgentProfilesConfig;
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
  /** #516 - workgroup label (e.g. "wg-5-dev-team"), "Root agent" for root-agent
   * groups, or `null` for non-WG / ad-hoc / unparseable launches. Always present. */
  workgroup: string | null;
  /** #516 - bare agent name (e.g. "dev-rust"), or `null` when the launch cwd
   * carries no replica identity. Always present. */
  agent: string | null;
  /** #566 - project folder name (e.g. "AgentsCommander_ac"), or `null` for
   * origin / ad-hoc / unparseable launches. Always present. */
  project: string | null;
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

export type UiAutomationAction =
  | "query"
  | "click"
  | "contextClick"
  | "setValue"
  | "typeText"
  | "backend";

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
  metadata: Record<string, string>;
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
  /** #384: per-replica stable coding-agent selection (`tooling.currentCodingAgent`). */
  currentCodingAgentId?: string;
  /** #384: per-replica profile letter (`tooling.profile`, then legacy override). */
  currentProfile?: string;
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

export type LoopTriggerKind = "cron";
export type LoopTargetKind = "workgroupCoordinator";
export type MissedWhileClosedPolicy = "notify";
export type BusyCoordinatorPolicy = "waitUntilIdle" | "forceInject" | "skip";

export interface LoopLastResult {
  kind: string;
  message: string;
}

export interface AcLoopSummary {
  id: string;
  name: string;
  enabled: boolean;
  expr: string;
  timezone: string;
  targetKind: LoopTargetKind;
  workgroup: string;
  promptPreview: string;
  busyCoordinator: BusyCoordinatorPolicy;
  path: string;
  configPath: string;
  lastCheckedAt: string | null;
  lastDueAt: string | null;
  lastDeliveredAt: string | null;
  lastResult: LoopLastResult | null;
  pendingDueAt: string | null;
  lastMissedClosedAt: string | null;
  nextDueAt: string | null;
}

export interface LoopConfigDetails {
  summary: AcLoopSummary;
  promptBody: string;
}

export interface LoopCreateInput {
  id?: string | null;
  name: string;
  expr: string;
  workgroup: string;
  promptBody: string;
  busyCoordinator?: BusyCoordinatorPolicy | null;
  enabled?: boolean | null;
}

export interface LoopUpdateInput {
  name?: string | null;
  expr?: string | null;
  workgroup?: string | null;
  promptBody?: string | null;
  busyCoordinator?: BusyCoordinatorPolicy | null;
  enabled?: boolean | null;
}

export interface LoopCronPreview {
  nextDueAt: string | null;
  upcoming: string[];
}

export interface LoopEventPayload {
  kind: string;
  projectPath: string;
  loopId: string;
  summary?: AcLoopSummary | null;
  message?: string | null;
}

export interface AcDiscoveryResult {
  agents: AcAgentMatrix[];
  teams: AcTeam[];
  workgroups: AcWorkgroup[];
  loops: AcLoopSummary[];
}

// ---------------------------------------------------------------------------
// Broad-scope coding-agent profile assignment (#384 §7)
// Mirrors src-tauri/src/commands/config.rs DTOs (all camelCase via serde).
// ---------------------------------------------------------------------------

/** `"replica"` | `"kind"` | `"workgroup"` — matches Rust `ProfileAssignmentScope`. */
export type ProfileAssignmentScope = "replica" | "kind" | "workgroup";

export interface ProfileAssignmentTarget {
  workgroupName: string;
  workgroupPath: string;
  replicaName: string;
  replicaPath: string;
  identityPath: string;
  originProject: string | null;
  /** Every live session whose working directory resolves to this replica. */
  liveSessionIds: string[];
}

export interface PreviewCodingAgentProfileSelectionRequest {
  targetReplicaPath: string;
  codingAgentId: string;
  profile: string;
  scope: ProfileAssignmentScope;
  /**
   * #384: the preview `targetFingerprint` is hashed over `restartSessions` (plan
   * §7, test #29), and apply re-validates that fingerprint — so the preview must
   * carry the restart choice or the two fingerprints can never match. The §7
   * Rust DTO listing omits this field; backend must include `restart_sessions`
   * here for the fingerprint to be consistent. (Flagged to tech-lead/dev-rust.)
   */
  restartSessions: boolean;
}

export interface PreviewCodingAgentProfileSelectionResult {
  scope: ProfileAssignmentScope;
  targetCount: number;
  liveSessionCount: number;
  /** Hash of (codingAgentId, profile, restart, sorted canonical target paths). */
  targetFingerprint: string;
  /** True for `kind`; requires the typed confirmation phrase. */
  requiresExplicitConfirmation: boolean;
  targets: ProfileAssignmentTarget[];
  warnings: string[];
}

export interface ApplyCodingAgentProfileSelectionRequest {
  targetReplicaPath: string;
  codingAgentId: string;
  profile: string;
  scope: ProfileAssignmentScope;
  restartSessions: boolean;
  /** Required for `kind`/`workgroup`; `null` allowed for single-target `replica`. */
  confirmedTargetFingerprint?: string | null;
  /** Required for `kind`; the exact typed phrase from the preview. */
  typedConfirmation?: string | null;
}

export interface ProfileAssignmentError {
  code: string;
  message: string;
  sessionIds: string[];
  replicaPaths: string[];
}

export interface ApplyCodingAgentProfileSelectionResult {
  scope: ProfileAssignmentScope;
  updatedCount: number;
  restartedCount: number;
  updatedReplicaPaths: string[];
  restartedSessionIds: string[];
  /** Sessions destroyed during restart that could not be recreated — surfaced as errors. */
  destroyedButNotRecreatedSessionIds: string[];
  targetFingerprint: string;
  warnings: string[];
  errors: ProfileAssignmentError[];
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


