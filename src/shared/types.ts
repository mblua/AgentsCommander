export interface SessionRepo {
  label: string;
  sourcePath: string;
  branch: string | null;
  dirty: boolean | null;
}

export type RepoBranchByPath = Record<string, string | null>;

export type RepoDirtyByPath = Record<string, boolean | null>;

export type SessionCommunicationKind = "raiseHand";

export interface SessionCommunication {
  kind: SessionCommunicationKind;
  visible: boolean;
  updatedAt: string;
}

export type SessionBackendKind = "localProcess" | "containerTransport";

export interface Session {
  id: string;
  name: string;
  shell: string;
  shellArgs: string[];
  backendKind?: SessionBackendKind;
  effectiveShellArgs: string[] | null;
  createdAt: string;
  workingDirectory: string;
  status: SessionStatus;
  waitingForInput: boolean;
  communication?: SessionCommunication | null;
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
  profileOutdated?: boolean;
}

export type SessionStatus = "active" | "running" | "idle" | { exited: number };

export type SessionSelectionMode = "none" | "live" | "dormant";

export type SessionSelectionCause =
  | { source: "initialHydration"; userInitiated: false; mode: "none" }
  | { source: "sessionCreated"; userInitiated: boolean; mode: "live" }
  | { source: "userSwitch"; userInitiated: true; mode: "live" }
  | { source: "userSwitch"; userInitiated: true; mode: "dormant" }
  | { source: "manualClose"; userInitiated: true; mode: "live" }
  | { source: "manualClose"; userInitiated: true; mode: "none" }
  | { source: "autoClose"; userInitiated: false; mode: "none" }
  | { source: "restart"; userInitiated: boolean; mode: "live" }
  | { source: "restart"; userInitiated: boolean; mode: "none" }
  | { source: "restore"; userInitiated: false; mode: "live" }
  | { source: "restore"; userInitiated: false; mode: "dormant" }
  | { source: "restore"; userInitiated: false; mode: "none" }
  | { source: "detach"; userInitiated: true; mode: "live" }
  | { source: "detach"; userInitiated: true; mode: "none" }
  | { source: "attach"; userInitiated: true; mode: "live" }
  | { source: "attach"; userInitiated: true; mode: "dormant" }
  | { source: "spawnRollback"; userInitiated: false; mode: "none" }
  | { source: "resourceMonitor"; userInitiated: boolean; mode: "none" }
  | { source: "backgroundCleanup"; userInitiated: false; mode: "none" }
  | { source: "livenessReconcile"; userInitiated: false; mode: "dormant" }
  | { source: "livenessReconcile"; userInitiated: false; mode: "none" };

export type SessionSelectionSource = SessionSelectionCause["source"];

interface SessionSelectionOrder {
  epoch: string;
  revision: number;
}

type SessionSelectionBase = SessionSelectionOrder & SessionSelectionCause;

type SessionSelectionData =
  | {
      mode: "none";
      id: null;
      status: null;
      hasPty: false;
      detached: false;
      displayable: false;
    }
  | {
      mode: "live";
      id: string;
      status: "active";
      hasPty: true;
      detached: false;
      displayable: true;
    }
  | {
      mode: "dormant";
      id: string;
      status: { exited: number };
      hasPty: boolean;
      detached: false;
      displayable: false;
    };

/**
 * The authoritative central selection. The intersection deliberately rejects
 * impossible source/mode/user-intent and mode/liveness combinations at compile
 * time; untrusted transport values enter through decodeSessionSelection.
 */
export type SessionSelection = SessionSelectionBase & SessionSelectionData;

export type CodingAgentKind = "claude" | "codex" | "gemini" | "pi";

export interface SessionContextPayload {
  sessionId: string;
  percent: number | null;
}

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
  sequence?: number;
}

export interface PtyViewport {
  cols: number;
  rows: number;
}

export interface PtyScreenSnapshot {
  sessionId: string;
  data: number[];
  rows: number | null;
  cols: number | null;
  sequence: number;
}

export interface ConfigSeedConfig {
  enabled: boolean;
  dest: string;
}

export interface AgentBackendConfig {
  kind?: SessionBackendKind;
  image?: string;
}

export interface AgentConfig {
  id: string;
  label: string;
  command: string;
  color: string;
  envs: CodingAgentEnv[];
  isolatedHome: boolean;
  instructionsFilename?: string;
  configSeed?: ConfigSeedConfig;
  contextRegex?: string;
  backend?: AgentBackendConfig;
}

export type CodingAgentEnvSource = "user" | "system";

export interface CodingAgentEnv {
  key: string;
  value: string;
  source: CodingAgentEnvSource;
  enabled: boolean;
}

export interface CodingAgentDefinition {
  key: string;
  label: string;
  description: string;
  color: string;
  command: string;
  instructionsFilename?: string;
  envs: CodingAgentEnv[];
  isolatedHome: boolean;
  configSeed?: ConfigSeedConfig;
  removable: boolean;
}

export interface ReseedResult {
  dest: string;
  backupPath: string;
}

export interface CodingAgentProfilesConfig {
  schemaVersion: number;
  profileSlots: Record<string, ProfileSlotConfig>;
  defaultProfileByAgent: Record<string, string>;
  profilesByAgent: Record<string, Record<string, ProfileCellConfig>>;
  profileLabelsByAgent: Record<string, Record<string, string>>;
}

export interface ProfileSlotConfig {
  label: string;
}

export interface ProfileCellConfig {
  enabled: boolean;
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

export const SESSION_WARNING_KINDS = [
  "container-path-in-host-field",
  "outside-mount",
  "no-value",
  "protocol-mismatch",
] as const;

export type SessionWarningKind = (typeof SESSION_WARNING_KINDS)[number];

export interface SessionWarning {
  sessionId: string;
  key: string;
  kind: SessionWarningKind;
  message: string;
}

export type SessionEnvWarningPayload = SessionWarning;

export interface WindowGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ScreenshotOverlayState {
  captureId: string;
  monitorId: number;
  monitorX: number;
  monitorY: number;
  width: number;
  height: number;
  scaleFactor: number;
  imageDataUrl: string;
  sessionId: string;
  sessionName: string;
  targetDirectory: string;
}

export interface ScreenshotSelection {
  captureId: string;
  monitorId: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ScreenshotCaptureResult {
  path: string;
  sessionId: string;
  sessionName: string;
}

export interface ScreenshotCaptureFailedEvent {
  message: string;
}

export interface ScreenshotHotkeyStatus {
  configured: string;
  registered: boolean;
  error: string | null;
}

export type MainSidebarSide = "left" | "right";

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export type WebServerOwnershipState =
  | "ownedRunning"
  | "externalListening"
  | "stopped";

export interface WebServerOwnedStatus {
  listening: boolean;
  owned: boolean;
  externalListening: boolean;
  openAllowed: boolean;
  bind: string;
  port: number;
  state: WebServerOwnershipState;
}

export type ApiClientMintScope = "send" | "list-peers-lean" | "session-transport";

export interface ApiClientMintRequest {
  root: string;
  scopes: ApiClientMintScope[];
  label?: string | null;
  expires?: string | null;
}

export interface ApiClientMintResponse {
  clientId: string;
  token: string;
  boundFqn: string;
  boundRoot: string;
  scopes: string[];
  expiresAt: string | null;
  note: string;
}

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
  mainResourceMonitorAttached: boolean;
  webServerEnabled: boolean;
  webServerPort: number;
  webServerBind: string;
  apiServerEnabled: boolean;
  apiServerPort: number;
  apiServerBind: string;
  projectPath: string | null;
  projectPaths: string[];
  archivedProjectPaths: string[];
  sidebarStyle: string;
  onboardingDismissed: boolean;
  coordSortByActivity: boolean;
  alwaysShowSelectedWorkgroup?: boolean;
  autoGenerateTaskTitle: boolean;
  agentTemplatesPath: string | null;
  themeLight: boolean;
  railCollapsedProjects?: string[];
  railFavoritesCollapsed?: boolean;
  specBoardEnabled: boolean;
  resourceMonitorEnabled: boolean;
  maxConcurrentAgentProcesses: number;
  resourceWatchdogAction: ResourceWatchdogAction;
  agentGroupWarnPrivateBytes: number;
  agentGroupKillPrivateBytes: number;
  agentProcessKillPrivateBytes: number;
  resourceKeepLastSnapshot: boolean;
  resourceBackoffPolling: boolean;
  coordinatorIdleBadgeYellowMinutes: number;
  coordinatorIdleBadgeRedMinutes: number;
  coordinatorAutoCloseEnabled: boolean;
  coordinatorAutoCloseMinutes: number;
  coordinatorAutoCloseSkipTelegramAssigned: boolean;
  coordinatorCascadeCloseEnabled: boolean;
  npmUpdateNotificationsEnabled: boolean;
  autoSelfClearEnabled: boolean;
  autoSelfClearByAgent: Record<string, boolean>;
  containerCredentialsFromHost: boolean;
  logLevel: LogLevel | null;
  screenshotCaptureHotkey?: string;
}

export interface UpdateInfo {
  currentVersion: string;
  latestVersion: string;
  upgradeCommand: string;
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
  workgroup: string | null;
  agent: string | null;
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
  blockedBySecurity: boolean;
  finalized: boolean;
}

export type UiAutomationAction =
  | "query"
  | "click"
  | "contextClick"
  | "hover"
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
  hover?: {
    from: string | null;
    to: string | null;
    changed: boolean;
    staleFrom?: boolean;
    reason?: "not_hovered";
    events: string[];
  };
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

export interface TeamSessionGroup {
  team: Team;
  coordinator: Session | null;
  members: Session[];
}


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

export interface SessionsState {
  sessions: Session[];
  activeId: string | null;
  selection: SessionSelection | null;
  selectionEpoch: string | null;
  selectionRevision: number;
  selectionConnectionGeneration: number | null;
  retiredSelectionEpochs: string[];
  connectionGeneration: number;
  transportConnected: boolean;
  awaitingHydrationGeneration: number | null;
  teams: Team[];
  teamFilter: string | null;
  showInactive: boolean;
  showCategories: boolean;
  alwaysShowSelectedWorkgroup: boolean;
  repos: RepoMatch[];
  coordSortByActivity: boolean;
  lastActivityBySessionId: Record<string, number>;
  contextPercentBySessionId: Record<string, number | null>;
  hydrated: boolean;
}


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
  currentCodingAgentId?: string;
  currentProfile?: string;
  lastUserMessageAt?: string;
  autoClosedAt?: string;
  manuallyClosedAt?: string;
}

export interface CoordinatorCloseOutcome {
  closed: boolean;
  workingCount: number;
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

export interface WorkgroupGroup {
  id: string;
  name: string;
  regex: string;
  favorite?: boolean;
}

export interface NonStopTelegramConfig {
  enabled: boolean;
  botId?: string | null;
}

export interface NonStopSoundConfig {
  enabled: boolean;
  seconds: number;
}

export interface NonStopGroupConfig {
  show: boolean;
  name: string;
  regex: string;
  toleranceSeconds: number;
  telegram: NonStopTelegramConfig;
  sound: NonStopSoundConfig;
}

export interface WorkgroupGroupsConfig {
  groups: WorkgroupGroup[];
  showAll: boolean;
  showUngrouped: boolean;
  nonStop?: NonStopGroupConfig | null;
}

export interface ProjectGroupsUpdatedPayload {
  projectPath: string;
  config: WorkgroupGroupsConfig;
}

export interface NonStopReport {
  projectPath: string;
  groupName: string;
  disparity: boolean;
  working: number;
  total: number;
  notWorkingWorkgroups: string[];
  toleranceSeconds: number;
  telegramEnabled: boolean;
  telegramBotId?: string | null;
  soundEnabled: boolean;
  soundSeconds: number;
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

export interface ContextTemplateUpdate {
  projectPath: string;
  workspacePath: string;
  filePath: string;
  filename: string;
  label: string;
  currentFileSha256: string;
  currentDefaultSha256: string;
  currentDefaultVersion: number;
}

export interface ContextTemplateOverwriteResult {
  filePath: string;
  backupPath: string;
  currentDefaultSha256: string;
}

export interface AcDiscoveryResult {
  agents: AcAgentMatrix[];
  teams: AcTeam[];
  workgroups: AcWorkgroup[];
  loops: AcLoopSummary[];
  contextTemplateUpdates: ContextTemplateUpdate[];
}


export type ProfileAssignmentScope = "replica" | "kind" | "workgroup";

export interface ProfileAssignmentTarget {
  workgroupName: string;
  workgroupPath: string;
  replicaName: string;
  replicaPath: string;
  identityPath: string;
  originProject: string | null;
  liveSessionIds: string[];
}

export interface PreviewCodingAgentProfileSelectionRequest {
  targetReplicaPath: string;
  codingAgentId: string;
  profile: string;
  scope: ProfileAssignmentScope;
  restartSessions: boolean;
}

export interface PreviewCodingAgentProfileSelectionResult {
  scope: ProfileAssignmentScope;
  targetCount: number;
  liveSessionCount: number;
  targetFingerprint: string;
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
  confirmedTargetFingerprint?: string | null;
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
  contextAlertPercentages: number[];
}


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


export interface ProjectRegistration {
  path: string;
  registered: boolean;
  created: boolean;
}

export interface ArchivedProject {
  path: string;
  folderName: string;
  exists: boolean;
  hasWorkspace: boolean;
}

export type ArchiveChangeReason =
  | "archive"
  | "unarchive"
  | "autoUnarchive"
  | "open"
  | "remove";

export interface ProjectArchiveChanged {
  path: string;
  folderName: string;
  archived: boolean;
  reason: ArchiveChangeReason;
  sessionName?: string;
}


export interface ErrorLogEntry {
  timestamp: string;
  level: string;
  target: string;
  message: string;
}


export interface RoleTemplateMeta {
  id: string;
  source: "agency" | "local";
  name: string;
  description: string;
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


