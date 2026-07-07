export interface SessionRepo {
  label: string;
  sourcePath: string;
  branch: string | null;
}

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
  /** #592 - backend-computed: loaded profile cell != current config. */
  profileOutdated?: boolean;
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
  sequence?: number;
}

export interface PtyScreenSnapshot {
  sessionId: string;
  data: number[];
  rows: number | null;
  cols: number | null;
  sequence: number;
}

/**
 * #598 — per-agent config-folder seed. Mirrors the Rust `ConfigSeedConfig`
 * (`#[serde(rename_all = "camelCase")]`): `enabled` (serde default true) and
 * `dest` (serde default ""). Lives on `AgentConfig` ONLY — nothing on
 * `ProfileCellConfig` / `CodingAgentProfilesConfig`.
 */
export interface ConfigSeedConfig {
  enabled: boolean;
  dest: string;
}

export interface AgentBackendConfig {
  kind?: SessionBackendKind;
}

export interface AgentConfig {
  id: string;
  label: string;
  command: string;
  color: string;
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
  /**
   * #598 — optional config-folder seed. When `enabled` and `dest` is non-empty,
   * AC copies a template config folder (chosen by convention, precedence
   * profile > matrix > coding-agent base) into the replica at spawn,
   * overwriting it every launch. `dest` is the destination folder NAME under
   * the replica root (e.g. ".claude"). Dropped to omitted before save when
   * disabled or `dest` is empty (mirrors `instructionsFilename`), so an
   * inactive seed is never persisted as a sentinel.
   */
  configSeed?: ConfigSeedConfig;
  backend?: AgentBackendConfig;
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

/**
 * #769 — a built-in coding-agent catalog entry, returned by the backend
 * `get_coding_agent_catalog` command (source of truth: the seeded, user-editable
 * `<config_dir>/coding-agents/agents.json`). Replaces the old hardcoded
 * `AGENT_PRESETS`. Maps onto `Omit<AgentConfig, "id">` plus `{ key, description,
 * removable }`; `definitionToSeed()` performs that projection for the "+ Add"
 * flow. `envs`, `isolatedHome`, and `removable` are always present (the backend
 * fills serde defaults); `instructionsFilename` / `configSeed` are omitted from
 * the JSON when unset.
 */
export interface CodingAgentDefinition {
  /** Stable catalog identity, `^[a-z0-9-]+$`, unique; doubles as a testid/CSS token. */
  key: string;
  label: string;
  /** "Coding Agent by …" subtitle shown on the onboarding card. */
  description: string;
  color: string;
  command: string;
  instructionsFilename?: string;
  envs: CodingAgentEnv[];
  isolatedHome: boolean;
  configSeed?: ConfigSeedConfig;
  /** Whether the user may delete this built-in (all Phase-1 built-ins are true). */
  removable: boolean;
}

/**
 * #769 Phase 2 — result of `reseed_coding_agent_default`: the config-folder
 * `dest` that was restored to the shipped default, plus the absolute path of the
 * timestamped `.bak` of the prior master (empty string when the master was absent
 * so nothing needed backing up).
 */
export interface ReseedResult {
  dest: string;
  backupPath: string;
}

export interface CodingAgentProfilesConfig {
  schemaVersion: number;
  /** v2 (#384): renamed from `letters`. Keyed by profile slot letter A–Z. */
  profileSlots: Record<string, ProfileSlotConfig>;
  /** v2 (#384): renamed from `agentDefaults`. agentId → default profile letter. */
  defaultProfileByAgent: Record<string, string>;
  /** v2 (#384): renamed from `matrix`. agentId → letter → cell. */
  profilesByAgent: Record<string, Record<string, ProfileCellConfig>>;
  /** #548: agentId → letter → override label. Empty/absent for a cell means
   *  inherit (primigenio label, else the legacy slot label, else the letter). */
  profileLabelsByAgent: Record<string, Record<string, string>>;
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

/** #714 Frozen monitor image + metadata handed to one screenshot-overlay window.
 *  Mirrors the Rust `ScreenshotOverlayState` (serde camelCase). `width`/`height`
 *  are PHYSICAL image pixels (the captured bitmap), which the overlay maps
 *  pointer coordinates onto proportionally. */
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

/** #714 Physical, monitor-local crop rectangle sent to the backend on release.
 *  Mirrors the Rust `ScreenshotSelection`. */
export interface ScreenshotSelection {
  captureId: string;
  monitorId: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** #714 Success payload for `screenshot_capture_saved` + the confirm command. */
export interface ScreenshotCaptureResult {
  path: string;
  sessionId: string;
  sessionName: string;
}

/** #714 Failure payload for the `screenshot_capture_failed` event. */
export interface ScreenshotCaptureFailedEvent {
  message: string;
}

/** #714 Global-hotkey registration status. `error` is null when registered (or
 *  not yet attempted). Mirrors the Rust `ScreenshotHotkeyStatus`. */
export interface ScreenshotHotkeyStatus {
  configured: string;
  registered: boolean;
  error: string | null;
}

export type MainSidebarSide = "left" | "right";

/** #612 LIVE log verbosity for `agentscommander*` targets. The 5 canonical
 *  lowercase wire values shared with the Rust side (`log_level: Option<String>`)
 *  and the `log_level_changed` event payload. Defined once here and imported by
 *  `console-capture.ts` / `ipc.ts` / `SettingsModal.tsx`. */
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
  // #587 — whether the Resource Monitor occupies the main central pane (vs the
  // terminal). Restored on startup; default false (terminal).
  mainResourceMonitorAttached: boolean;
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
  /** #552 coordinator idle-badge color thresholds, in minutes. Mirror of the
   *  Rust `coordinator_idle_badge_*_minutes` fields (camelCase via serde).
   *  Color helper requires yellow < red (validated at Settings save time). */
  coordinatorIdleBadgeYellowMinutes: number;
  coordinatorIdleBadgeRedMinutes: number;
  /** #552 auto-close lifecycle clock: when enabled, a team whose sessions go
   *  fully silent for `coordinatorAutoCloseMinutes` is terminated. */
  coordinatorAutoCloseEnabled: boolean;
  coordinatorAutoCloseMinutes: number;
  /** #817 When true, background auto-close skips sessions with a Telegram
   *  assignment. Default false preserves legacy behavior. */
  coordinatorAutoCloseSkipTelegramAssigned: boolean;
  /** #588 When true, manually closing a coordinator also closes its team. */
  coordinatorCascadeCloseEnabled: boolean;
  /** #609 Check npm on startup (<=1x/24h) and notify when a newer published
   *  version of @mblua/agentscommander is available. Default true. */
  npmUpdateNotificationsEnabled: boolean;
  /** #640 Global master for auto self-handoff-and-clear. ON => class default
   *  applies (coordinator/Root on, specialists off). Per-agent overrides in
   *  autoSelfClearByAgent. */
  autoSelfClearEnabled: boolean;
  /** #640 Per-agent override of the class default, keyed by agent name. */
  autoSelfClearByAgent: Record<string, boolean>;
  /** #612 LIVE log level for agentscommander targets. null (legacy/unset) => "info". */
  logLevel: LogLevel | null;
  /** #714 Native global hotkey for screenshot capture (e.g. "Ctrl+Q"). Optional
   *  here only to ease partial-settings test construction; the Rust
   *  `#[serde(default)]` always emits it, so it is present at runtime. */
  screenshotCaptureHotkey?: string;
}

/** #609 "npm update available" payload. Mirrors the Rust `UpdateInfo` struct
 *  (serde camelCase): carried by the `npm_update_available` event and returned
 *  by `get_update_status` (null when up-to-date / not yet checked / disabled). */
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
  /**
   * #647 D: true when the kill quarantined AND a failure carried the exact
   * ACCESS_DENIED code (`win32 error 5`) — a security product is stripping
   * PROCESS_TERMINATE. The per-PID detail stays in `message`; this only ADDS
   * the AV-exclusion guidance in the UI, so a non-security failure is never
   * hidden. Mirrors Rust `ResourceKillResult.blocked_by_security` (serde
   * camelCase). `#[serde(default)]` backend-side, so always present on the wire.
   */
  blockedBySecurity: boolean;
  /**
   * #647 (Step 7): true ONLY when `kill_resource_group` verified the tree dead,
   * tore down the PTY/job, and flipped the tile to Exited. Success keys off THIS,
   * NOT `!quarantined`: a `Terminating` early-return (a concurrent kill still
   * settling) reports `quarantined === false` but is NOT a finalized success, so
   * treating it as one would close the modal over a still-Running zombie tile.
   * Mirrors Rust `ResourceKillResult.finalized` (`#[serde(default)]`).
   */
  finalized: boolean;
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
  /** #552/#580 RFC3339 timestamp of the unified team-idle anchor, i.e. the
   *  backend's `max(last_user_message_at, last_activity_at)` — the field now
   *  means "team idle since" (reset when you message the coordinator, any member
   *  is active, or the coordinator is active), NOT just the user's last message
   *  (#580; rename to idleSinceAt deferred). `undefined` when none. Only
   *  meaningful when `isCoordinator`. Drives the idle badge; read from the
   *  persisted CoordinatorClocks store (survives restart + dormant). */
  lastUserMessageAt?: string;
  /** #552 RFC3339 time this coordinator's team was auto-closed for inactivity,
   *  or undefined. Only meaningful when `isCoordinator`. Drives the neutral
   *  "auto-closed" pill; cleared on reopen. */
  autoClosedAt?: string;
  /** #588 RFC3339 time this coordinator was manually closed, or undefined. Only
   *  meaningful when `isCoordinator`. Drives the MANUALLY-CLOSED pill; cleared
   *  on reopen. Visually identical to the auto-closed pill, different label. */
  manuallyClosedAt?: string;
}

/** #588 Result of the `close_coordinator` command. When `closed` is false the
 *  backend refused to cascade-close a coordinator whose team has working members
 *  without confirmation; `workingCount` is how many are still working. */
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
}

export interface NonStopTelegramConfig {
  enabled: boolean;
  /** Resolves against AppSettings.telegramBots by id; null/absent => first configured bot. */
  botId?: string | null;
}

export interface NonStopSoundConfig {
  enabled: boolean;
  /** Beep duration in seconds. Default 3. Clamped 1..=60. */
  seconds: number;
}

export interface NonStopGroupConfig {
  /** Rail visibility AND watchdog-active. Single toggle (#777 D3/D4). Default false. */
  show: boolean;
  /** Display name. Default "Alert me!". */
  name: string;
  /** Membership regex, dynamic like user groups. Default "(?!)" (matches nothing). */
  regex: string;
  /** Grace window before firing. Default 30. Clamped 1..=3600. */
  toleranceSeconds: number;
  telegram: NonStopTelegramConfig;
  sound: NonStopSoundConfig;
}

export interface WorkgroupGroupsConfig {
  groups: WorkgroupGroup[];
  showAll: boolean;
  showUngrouped: boolean;
  /** #777 built-in optional Non-stop group. Absent on legacy configs. */
  nonStop?: NonStopGroupConfig | null;
}

export interface ProjectGroupsUpdatedPayload {
  projectPath: string;
  config: WorkgroupGroupsConfig;
}

/** #777 frontend -> backend watchdog signal; one entry per project with an ACTIVE Non-stop group. */
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
  /** Backend confirmation hint; frontend broad scopes use a checkbox confirmation gate. */
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
  /** Legacy typed confirmation field; frontend sends `null` and relies on fingerprint + checkbox confirmation. */
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


