export interface SessionRepo {
  label: string;
  sourcePath: string;
  branch: string | null;
  dirty: boolean | null;
}

export type RepoBranchByPath = Record<string, string | null>;

export type RepoDirtyByPath = Record<string, boolean | null>;

export type SessionCommunicationKind = "raiseHand" | "blockedMenu";

export interface SessionCommunication {
  kind: SessionCommunicationKind;
  visible: boolean;
  updatedAt: string;
  message?: string | null;
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

export type CodingAgentKind = "claude" | "codex" | "pi" | "antigravity";

export interface SessionContextPayload {
  sessionId: string;
  percent: number | null;
}

/** #1682 - the instant of the most recent busy->idle edge on `sessionId` that the
 *  backend judged an agent turn, RFC3339/UTC as it stored it. It is the backend's
 *  proxy for the agent having finished responding, not a proof of it: an armed
 *  session that submitted nothing still produces this event. */
export interface SessionAgentMessagePayload {
  sessionId: string;
  at: string;
}

/** #1171 - what a watcher match means. Mirrors `WatcherMode` (`config/settings.rs`). */
export type WatcherMode = "state" | "occurrence";

/** #1171 - what makes two `occurrence` matches the same one inside the dedupe window. */
export type WatcherDedupe = "row" | "capture" | "none";

/**
 * #1171 - one watcher activation.
 *
 * Mirrors `WatcherMatchPayload` (`pty/watchers/mod.rs`), pinned field for field by
 * `the_payload_serializes_to_the_exact_camel_case_contract`. **No field is ever absent**:
 * the Rust struct carries no `skip_serializing_if`, so absent never becomes a third state
 * beside null and the value.
 */
export interface WatcherMatchPayload {
  sessionId: string;
  /**
   * Monotonic per session. The same value the ring stores, so the window merges snapshot
   * and stream on `(sessionId, seq)`. Two matches from one tick share `at` and are only
   * distinguishable by this.
   */
  seq: number;
  /** The key of the root `watchers` map. The same grouping key everywhere. */
  watcherId: string;
  mode: WatcherMode;
  /** RFC3339 UTC. The tick's instant, not the match's: a match has no instant of its own. */
  at: string;
  /** Groups 1..n in order, without group 0. `null` per element means "did not capture". */
  captures: (string | null)[];
  /** The logical row, truncated to 256 bytes on a char boundary. */
  row: string;
  /** Whether `row` lost bytes to the cap; `row.length` cannot answer it, the cap is on bytes. */
  rowTruncated: boolean;
}

/** #1171 - one tick's matches for one session, coalesced. Payload of `watcher_matches`. */
export interface WatcherMatchBatch {
  sessionId: string;
  matches: WatcherMatchPayload[];
}

/** #1171 - one watcher's standing on one session. Present even when `count` is 0. */
export interface WatcherActivityCounter {
  watcherId: string;
  mode: WatcherMode;
  count: number;
  /** True while this watcher is hitting a per-tick cap or is suspended. */
  degraded: boolean;
}

/** #1171 - everything `get_watcher_activity` answers with. */
export interface WatcherActivitySnapshot {
  /** Oldest first. `limit` trims from the new end: the n most recent, still oldest first. */
  matches: WatcherMatchPayload[];
  /** The highest `seq` ever inserted for this session; the merge fence against the stream. */
  lastSeq: number;
  /** The ring dropped at least one entry since the session started. */
  truncated: boolean;
  /** Monotonic since the session started. NOT a count of lost matches. */
  possiblyMissedFrames: number;
  /**
   * False until the engine has ticked this session at least once. Without it an empty
   * `activeWatchers` cannot tell "no watcher reaches this agent" from "the engine has not
   * run yet".
   */
  warmedUp: boolean;
  activeWatchers: WatcherActivityCounter[];
}

/** #1171 - what a candidate pattern does, before it is saved. */
export interface WatcherPatternPreview {
  compiles: boolean;
  error: string | null;
  /**
   * False when no session was given, or the session had no readable frame. This is what
   * distinguishes "matched nothing" from "could not look".
   */
  sampled: boolean;
  matchedRows: number;
  totalRows: number;
  /** Up to 3 matched logical rows, each truncated to 256 bytes. */
  samples: string[];
  /**
   * True when the captures of the lowest match differed between two samples taken about a
   * second apart: a pattern capturing a clock matches one row and still emits constantly.
   */
  capturesVolatile: boolean;
}

/**
 * #1171 - one watcher row of the draft the Settings modal holds in memory.
 *
 * Only the three fields `reaches` and the budget depend on. `pattern`, `mode`, `dedupe` and
 * `capturedAgainst` take part in neither and are deliberately not sent: the row already shows
 * its pattern, and `previewWatcherPattern` answers compilability separately, so carrying it
 * here would inflate every debounced payload to restate an answer already on screen.
 */
export interface WatcherDraftEntry {
  id: string;
  enabled: boolean;
  commands?: string[] | null;
}

/**
 * #1171 - one agent row of the same draft.
 *
 * The modal edits agents and watchers in ONE store and one Save writes both, so resolving
 * against the SAVED agent list would answer about a state the user has already left. Two of
 * the three agent edits over-report that way: deleting an agent leaves it named in a reach
 * list it will not be in, and changing an agent's `command` leaves a watcher reported as
 * reaching it under the old stem. Only adding an agent under-reports.
 */
export interface WatcherAgentDraftEntry {
  id: string;
  label: string;
  command: string;
}

/** #1171 - one agent that a draft row's selector reaches. */
export interface WatcherReachEntry {
  agentId: string;
  agentLabel: string;
  commandStem: string;
  /**
   * Whether this row is enabled in the draft AND holds one of this agent's 8 slots once every
   * other enabled row of the draft is counted. It is slot assignment, **not** a promise that
   * the watcher will emit anything: a resolved watcher whose pattern does not compile is
   * allocated a slot and is inert, and compilability is answered separately by
   * `previewWatcherPattern`. A disabled row is always false here, and the editor, which owns
   * `enabled`, says "disabled" rather than "budget".
   */
  allocated: boolean;
}

/**
 * #1171 - the reach of one draft row.
 *
 * Exactly one per requested row, in request order. It carries `id` back because the editor
 * filters unrecognised rows out of the request, so its table positions do not match the
 * response positions. A row that reaches nobody is still present, with `entries: []`.
 */
export interface WatcherReachRow {
  id: string;
  /**
   * Every agent this row's selector reaches, whether or not the row is enabled: reach is a
   * property of the selector alone, and `allocated` is where enablement and budget land.
   * Ordered by `agentLabel` with `agentId` as the tie-break, so the list does not reshuffle
   * between keystrokes.
   */
  entries: WatcherReachEntry[];
}

/** #1171 - one user-configured watcher. Mirrors `WatcherConfig` (`config/settings.rs`). */
export interface WatcherConfig {
  enabled: boolean;
  mode: WatcherMode;
  pattern: string;
  /**
   * Absent or null reaches every configured agent; present reaches only entries whose
   * command executable stem matches exactly; `[]` reaches none. **Absent and `[]` are
   * opposites**, which is why this is not a plain `string[]`.
   */
  commands?: string[] | null;
  dedupe: WatcherDedupe;
  dedupeWindowMs: number;
  /** Free text, e.g. "claude 2.1.212". Never validated, never parsed. */
  capturedAgainst?: string | null;
}

/**
 * #1171 - one entry of the root `watchers` map, valid or not.
 *
 * Mirrors the untagged `WatcherEntry` (`config/settings.rs`), which exists so a hand-written
 * `"mode": "State"` skips one watcher instead of failing the whole `AppSettings` parse and
 * starting with no agents configured. The invalid value is kept verbatim so a save
 * round-trips the user's bytes; typing this map as `WatcherConfig` alone would claim a
 * guarantee the backend deliberately does not make, and an editor built on that claim would
 * delete what it could not read.
 */
export type WatcherEntry = WatcherConfig | UnrecognizedWatcherEntry;

/**
 * An entry that did not deserialize as a `WatcherConfig`, preserved verbatim.
 *
 * This is `serde_json::Value` and therefore **any** JSON value, not only an object: a
 * hand-written `"permission": "claude"`, `"permission": 7`, `"permission": null` or
 * `"permission": ["claude"]` all land here and are all written back unchanged. Modelling
 * only objects made the contract claim a narrowness Rust does not have, and forced every
 * test of a real case through `as unknown as WatcherEntry` -- a cast is what a type says
 * when it is wrong.
 */
export type UnrecognizedWatcherEntry = JsonValue;

// ── terminal output wire payload ──────────────────────────────────────

/**
 * #1363 - the flat `pty_output` payload, restored from the broadcast push
 * architecture (`4de8e11`). `sequence` is a NUMBER, not a canonical decimal
 * string: the reconcile is `event.sequence <= snapshot.sequence`, and a string
 * comparison there is lexicographic, so `"9" <= "10"` would be false and the
 * watermark would silently break past sequence 9.
 *
 * The field is absent when the backend's parser is unavailable; such a chunk
 * is written live with no reconcile (PR #961: live PTY bytes are never gated).
 */
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
  /** #1318/#1323 - per-agent update-command sequence seeded by AC: an ORDERED
   * array of COMPLETE command strings, each executed sequentially (e.g.
   * ["claude --update"] or ["claude --update", "npm i -g @scope/cli"]).
   * NOT argv tokens. Empty = no update command. Consumed by the #1327 startup
   * update pass (`build_update_plan`) and listed by the #1551 Settings
   * Auto-update table. */
  updateCommands: string[];
  /** #1318 - stable catalog default for auto-update; the per-user choice lives in AppSettings.agentAutoUpdateByCommand. Inert: the runtime reads only `AppSettings.agentAutoUpdateByCommand`. */
  autoUpdate: boolean;
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
  | "starting"
  | "ownedRunning"
  | "stopping"
  | "externalListening"
  | "stopped";

/** #1453 - reason the last web server start failed. `bind` is ALWAYS the raw
 *  settings string (never the canonical SocketAddr form), so a consumer that
 *  cross-checks it against `WebServerInterfaceInfo` must gate on an IPv4 shape
 *  first. `detail` is the verbatim OS error. */
export interface WebServerBindFailure {
  bind: string;
  port: number;
  detail: string;
}

/** #1453 - one IPv4 address currently assigned to an adapter, offerable as a
 *  bind. Loopback, link-local and unspecified are filtered out by the backend. */
export interface WebServerInterfaceInfo {
  address: string;
  interfaceName: string;
  isVirtual: boolean;
}

export interface WebServerOwnedStatus {
  listening: boolean;
  owned: boolean;
  externalListening: boolean;
  openAllowed: boolean;
  bind: string;
  port: number;
  state: WebServerOwnershipState;
  // Required, not optional: serde always emits the key, `null` when absent.
  bindFailure: WebServerBindFailure | null;
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
  terminalSnapshotsEnabled: boolean;
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
  gitSweepConcurrency: number;
  gitSweepMinIntervalSecs: number;
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
  /** #1327 - per-command auto-update policy, keyed by the catalog COMMAND
   * string (not the agent id: several profiles can share one command; the
   * software is the update unit). Absent = never asked (the startup dialog
   * asks once, default No); true = run this command's updateCommands at
   * startup; false = never ask again. Replaces the two inert #1318
   * agent-id-keyed maps. */
  agentAutoUpdateByCommand: Record<string, boolean>;
  containerCredentialsFromHost: boolean;
  logLevel: LogLevel | null;
  activityLogEnabled: boolean;
  screenshotCaptureHotkey?: string;
  /**
   * #1171 - root-level watcher patterns, keyed by watcher id. Optional because the Rust
   * field skips serializing while the map is empty, so a user who configures nothing never
   * sees the key appear.
   */
  watchers?: Record<string, WatcherEntry>;
  /** #1171 - geometry of the watcher activity window; skipped while unset. */
  watchersGeometry?: WindowGeometry;
}

// ── #1077 Portable dual project paths: get_settings resolution report ────────
// These mirror the serialize-only Rust wire shapes in
// `src-tauri/src/commands/config.rs` (SettingsSnapshot / ProjectPathIssue /
// Raw{String,Json}FieldState / ProjectPathReconciliationError), all camelCase.
// The report rides on SettingsAPI.get() but is transport-untrusted: it MUST be
// runtime-validated at its consumption boundary (see sidebar/stores/project.ts),
// never trusted on the strength of this transport generic alone.

/** A recursive JSON value: the shape of a wrong-typed structural candidate
 *  field carried through the report. Validated iteratively, never via `any`. */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

/** Tagged raw string field state. `present: false` always pairs with
 *  `value: null`; `present: true, value: null` is an explicit JSON null. */
export interface RawStringFieldState {
  present: boolean;
  value: string | null;
}

/** Tagged raw JSON field state (needed for wrong-typed structural fields and
 *  absent-vs-null parity). Same absent/null invariant as RawStringFieldState. */
export interface RawJsonFieldState {
  present: boolean;
  value: JsonValue | null;
}

export type ReconciliationStage = "read" | "write";

/** Transport/I-O reconciliation error only; structural/candidate issues live in
 *  `issues`, never here. `retryable` is always `true`. */
export interface ProjectPathReconciliationError {
  stage: ReconciliationStage;
  message: string;
  retryable: true;
}

export type ProjectPathIssueSource =
  | "projectPath"
  | "projectPaths"
  | "archivedProjectPaths";

/** Both stored locations resolved to different directories, so neither was
 *  selected. Both raw candidates and both resolved paths are always present. */
export interface ProjectPathConflictIssue {
  kind: "conflict";
  id: string;
  source: ProjectPathIssueSource;
  index?: number;
  absoluteCandidate: string;
  instanceRelativeCandidate: string;
  absoluteResolvedPath: string;
  instanceRelativeResolvedPath: string;
  message: string;
}

/** Every candidate side resolved to a definite NotFound. */
export interface ProjectPathMissingIssue {
  kind: "missing";
  id: string;
  source: ProjectPathIssueSource;
  index?: number;
  absoluteCandidate: RawStringFieldState;
  instanceRelativeCandidate: RawStringFieldState;
  absoluteResolvedPath: string | null;
  instanceRelativeResolvedPath: string | null;
  message: string;
}

/** Malformed syntax/type, permission/I-O failure, non-directory, missing `.ac`,
 *  structural corruption, non-UTF-8, quarantine, or identity failure. */
export interface ProjectPathInvalidIssue {
  kind: "invalid";
  id: string;
  source: ProjectPathIssueSource;
  index?: number;
  absoluteCandidate: RawJsonFieldState;
  instanceRelativeCandidate: RawJsonFieldState;
  absoluteResolvedPath: string | null;
  instanceRelativeResolvedPath: string | null;
  reason: string;
}

export type ProjectPathIssue =
  | ProjectPathConflictIssue
  | ProjectPathMissingIssue
  | ProjectPathInvalidIssue;

export interface ProjectPathResolution {
  activeRegistrationCount: number;
  archivedRegistrationCount: number;
  issues: ProjectPathIssue[];
  reconciliationError: ProjectPathReconciliationError | null;
}

/** The flattened `get_settings` response: the runtime-selected AppSettings plus
 *  the structured resolution report. */
export interface SettingsSnapshot extends AppSettings {
  projectPathResolution: ProjectPathResolution;
  /** #1347 - absolute path of the instance settings.json this snapshot came
   *  from, or null when the backend could not resolve its config dir. Read-only
   *  metadata: it is never edited, never part of a save payload. */
  settingsFilePath: string | null;
}

export interface UpdateInfo {
  currentVersion: string;
  latestVersion: string;
  upgradeCommand: string;
}

/** #1691 - why a command's update sequence ended. `ok` stays for the legacy predicate;
 *  `outcome` is the only truthful classification (a cancelled command has `ok=false`). */
export type AgentUpdateOutcome = "succeeded" | "failed" | "cancelled";

/** #1691 - what the post-update probe proved about the installed version. */
export type AgentUpdateChange = "changed" | "unchanged" | "unknown";

/** #1691 - what `agent_update_cancel` did with the request. */
export type AgentUpdateCancelDisposition =
  | "requested"
  | "already_requested"
  | "already_terminal"
  | "not_in_pass";

export interface AgentUpdateResult {
  command: string;
  label: string;
  ok: boolean;
  outcome: AgentUpdateOutcome;
  error?: string | null;
  /** #1691 - the pre-update probe, `null` when it never ran; the key is always present. */
  installBefore: InstallState | null;
  /** #1691 - the post-update probe, `null` when it never ran; the key is always present. */
  installAfter: InstallState | null;
  change: AgentUpdateChange;
  /** #1691 - the post-update probe's own diagnostic; omitted when the probe succeeded. */
  verificationError?: string;
}

/** #1691 - a result as it may arrive from an older backend: the four #1691 keys may be absent.
 *  `normalizeAgentUpdateResult` (sidebar/agent-update.ts) turns one of these into the canonical
 *  shape before any store fold or notification classification. */
export type AgentUpdateResultWire = Omit<
  AgentUpdateResult,
  "outcome" | "installBefore" | "installAfter" | "change"
> &
  Partial<Pick<AgentUpdateResult, "outcome" | "installBefore" | "installAfter" | "change">>;

/** #1691 - the response of `agent_update_cancel` for one command. */
export interface AgentUpdateCancelResponse {
  command: string;
  disposition: AgentUpdateCancelDisposition;
}

/** #1691 - the response of `agent_updates_cancel_all`, partitioned over the pass. */
export interface AgentUpdateCancelAllResponse {
  requested: AgentUpdateCommandRef[];
  alreadyRequested: AgentUpdateCommandRef[];
  alreadyTerminal: AgentUpdateCommandRef[];
}

/** #1691 - the full cancellation snapshot carried by `agent_update_cancellation_changed`. */
export interface AgentUpdateCancellationChanged {
  cancelRequested: AgentUpdateCommandRef[];
  cancelAllRequested: boolean;
}

export interface AgentUpdatePrompt {
  command: string;
  label: string;
}

/** #1551 - a command in the startup update pass (start order); also the payload of `agent_update_prompt_closed`. */
export interface AgentUpdateCommandRef {
  command: string;
  label: string;
}

export type InstallStatus = "checking" | "missing" | "installed" | "probeFailed" | "unprobed";

/** #1551 - resolved install state of one catalog command; `path`/`detail` absent when the backend has none.
 *  `seq` is the backend cache's commit counter (0 = `checking`); a higher `seq` is always the newer state. */
export interface InstallState {
  status: InstallStatus;
  version?: string | null;
  path?: string | null;
  detail?: string | null;
  seq: number;
}

/** #1551 - one update-capable catalog entry (catalog order, duplicates kept). Configured/registered/live facts are FE-derived. */
export interface AgentUpdateOverviewRow {
  key: string;
  label: string;
  command: string;
  color: string;
  updateCommands: string[];
  install: InstallState;
}

export interface AgentInstallStateChanged {
  command: string;
  install: InstallState;
}

/** #1551 round 5 - one agent of this boot's startup pass, in pass (catalog) order; also the payload of `agent_update_command_started`.
 *  `installBefore` is the pre-update probe result (absent until that probe ran; `seq` 0, never cached). */
export interface AgentUpdateNode {
  command: string;
  label: string;
  updateCommands: string[];
  installBefore?: InstallState | null;
}

export interface AgentUpdateStatus {
  inProgress: boolean;
  /** The currently displayed prompt (sequential phase: at most one);
   * restored from the snapshot by a late-mounting sidebar. */
  prompt: AgentUpdatePrompt | null;
  results: AgentUpdateResult[];
  /** #1551 - commands whose update sequence is running, in start order. */
  running: AgentUpdateCommandRef[];
  /** #1551 - policy recorded by the winning answer per prompted command this boot; absent on an older backend. */
  answered?: Record<string, boolean>;
  /** #1551 - the pass nodes in pass order; absent on an older backend. */
  nodes?: AgentUpdateNode[];
  /** #1691 - commands whose update sequence finished and whose post-update probe is running, in pass order. */
  verifying: AgentUpdateCommandRef[];
  /** #1691 - commands whose cancellation the backend has accepted, in pass order. */
  cancelRequested: AgentUpdateCommandRef[];
  /** #1691 - a batch cancellation was accepted for this pass. */
  cancelAllRequested: boolean;
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
  | "backend"
  | "terminal";

export interface UiAutomationRequest<
  A extends UiAutomationAction = Exclude<UiAutomationAction, "terminal">,
> {
  requestId: string;
  token: string;
  window: string;
  action: A;
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

export const MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT =
  "main-terminal-layout-pulse-request";

export type MainTerminalLayoutGeometry = {
  hostWidth: number;
  cols: number;
  rows: number;
};

export type MainTerminalLayoutObserverAck = {
  epoch: number;
  first: MainTerminalLayoutGeometry;
  second: MainTerminalLayoutGeometry;
};

export type MainTerminalLayoutPulseSample = MainTerminalLayoutGeometry & {
  observedObserverEpoch: number;
  completedObserverAck: MainTerminalLayoutObserverAck | null;
};

export type MainTerminalLayoutPulseStatus =
  | "completed"
  | "skipped"
  | "cancelled"
  | "failed";

export type MainTerminalLayoutPulseReason =
  | "completed"
  | "unhandled"
  | "busy"
  | "dragging"
  | "persistence_owned"
  | "invalid_sample"
  | "clamped"
  | "stale"
  | "width_changed"
  | "teardown"
  | "initialization_timeout"
  | "request_timeout"
  | "expanded_timeout"
  | "restore_timeout"
  | "exception";

export type MainTerminalLayoutPulsePhaseTrace = {
  sidebarWidth: number | null;
  hostWidth: number | null;
  cols: number | null;
  rows: number | null;
  baselineObservedEpoch: number | null;
  completedObserverAck: MainTerminalLayoutObserverAck | null;
};

export type MainTerminalLayoutPulseTrace = {
  version: 1;
  requestId: number;
  sessionId: string;
  attachGeneration: number;
  status: MainTerminalLayoutPulseStatus;
  reason: MainTerminalLayoutPulseReason;
  original: MainTerminalLayoutPulsePhaseTrace;
  expanded: MainTerminalLayoutPulsePhaseTrace;
  restored: MainTerminalLayoutPulsePhaseTrace;
  dwellMs: number;
  settingsWritesDelta: number;
};

export type MainTerminalLayoutPulseResult = {
  status: MainTerminalLayoutPulseStatus;
  reason: MainTerminalLayoutPulseReason;
  trace: MainTerminalLayoutPulseTrace;
};

export type MainTerminalLayoutPulseRequest = {
  requestId: number;
  sessionId: string;
  attachGeneration: number;
  accepted: boolean;
  sample: () => MainTerminalLayoutPulseSample | null;
  complete: (result: MainTerminalLayoutPulseResult) => void;
};

export interface UiTerminalAutomationTarget {
  sessionId: string;
  baseY: number;
  viewportY: number;
  length: number;
  cols: number;
  rows: number;
  type: "normal" | "alternate";
  atBottom: boolean;
  layoutPulse?: MainTerminalLayoutPulseTrace | null;
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

type UiAutomationSuccessResponse<A extends UiAutomationAction> =
  A extends "terminal"
    ? {
        ok: true;
        requestId: string;
        window: string;
        action: "terminal";
        selector: string;
        target: UiTerminalAutomationTarget;
        diagnostics?: UiAutomationDiagnostics;
      }
    : {
        ok: true;
        requestId: string;
        window: string;
        action: Exclude<A, "terminal">;
        selector: string;
        target: UiAutomationTarget;
        diagnostics?: UiAutomationDiagnostics;
      };

export type UiAutomationResponse<
  A extends UiAutomationAction = Exclude<UiAutomationAction, "terminal">,
> =
  | UiAutomationSuccessResponse<A>
  | {
      ok: false;
      requestId: string;
      window: string;
      action: A;
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
        | "terminal_controller_unavailable"
        | "terminal_target_mismatch"
        | "terminal_entry_stale"
        | "terminal_session_not_visible"
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
  favorite?: boolean;
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


