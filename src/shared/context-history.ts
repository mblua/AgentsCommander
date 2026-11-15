// #3532 P06 / #4091 - TypeScript mirror of the schema-version-3 context-history DTOs
// served by the four `get_context_history_*` commands. Field names, nulls and
// enum spellings follow the Rust serde output exactly; the frontend never
// recomputes a mean, a selection or a scope from these values.

export const CONTEXT_HISTORY_SCHEMA_VERSION = 3;

/** A launch-metadata filter: any value, an unknown (null) value, or one exact value. */
export type LaunchFilter =
  | { kind: "any" }
  | { kind: "unknown" }
  | { kind: "exact"; value: string };

export interface ReportQuery {
  projectPath: string;
  fromUtcMs: number;
  toUtcMs: number;
  agentKey: string | null;
  teamId: string | null;
  codingAgentId: LaunchFilter;
  model: LaunchFilter;
  effort: LaunchFilter;
  cursor: string | null;
  /** #4091 count mode: newest N sessions per scope before `toUtcMs`; null is date mode. */
  sessionCount?: number | null;
}

/** Changes of one logical session; `query.agentKey` is required. */
export interface ChangesRequest {
  query: ReportQuery;
  /** `SessionSummary.logicalSessionId`, never a runtime session id. */
  logicalSessionId: string;
}

/** 1..100 unique live runtime session ids of the registered project. */
export interface SidebarMeansQuery {
  projectPath: string;
  runtimeSessionIds: string[];
}

export type MatrixType = "replica" | "origin";
export type TeamAttribution = "known" | "unknown" | "ambiguous";
export type MetadataSource = "launch_command" | "unavailable";
export type RecordKind = "change" | "boundary";
export type BoundaryReason = "observed_zero" | "lifecycle" | "observation_gap";
export type InitialKind = "confirmed_zero_transition" | "first_observed" | "unknown";

export interface RequestedPeriod {
  fromUtcMs: number;
  toUtcMs: number;
  /** #4091 Requested count in count mode; null in date mode. */
  sessionCount: number | null;
  selection: "recent_sessions" | "session_started";
}

export interface Coverage {
  retentionDays: number;
  retainedEarliestUtcMs: number | null;
  retainedLatestUtcMs: number | null;
  incomplete: boolean;
  unknownStartSessionsCount: number;
}

export interface ResponseHeader {
  schemaVersion: number;
  requestedPeriod: RequestedPeriod;
  generatedAtUtcMs: number;
  coverage: Coverage;
}

export interface SessionMetadata {
  codingAgentId: string | null;
  model: string | null;
  effort: string | null;
  metadataSource: MetadataSource;
  teamAttribution: TeamAttribution;
  teamIds: string[];
}

export interface MetadataBucket extends SessionMetadata {
  sessionsCount: number;
}

export interface Aggregate {
  recordsCount: number;
  positiveSum: number;
  mean: number | null;
  sessionsCount: number;
  /** #4091 `recordsCount / sessionsCount` from the backend; never derived here. */
  recordsPerSession: number | null;
  incomplete: boolean;
}

/** #4091 Requested versus available sessions of one scope in count mode. */
export interface ScopeWindowCoverage {
  teamAttribution: TeamAttribution;
  teamIds: string[];
  requestedSessions: number;
  availableSessions: number;
}

/** #4091 The newest `requestedSessions` sessions of a known scope. */
export interface SessionWindowBucket extends Aggregate {
  requestedSessions: number;
}

export interface AgentSummary extends Aggregate {
  agentKey: string;
  agentName: string;
  metadataBuckets: MetadataBucket[];
  /** Count mode only; null in date mode. */
  scopeWindows: ScopeWindowCoverage[] | null;
}

export interface ReportResponse extends ResponseHeader {
  agentsPage: AgentSummary[];
  nextAgentCursor: string | null;
  totalAgents: number;
}

export interface SessionStats {
  boundaryReason: BoundaryReason | null;
  recordsCount: number;
  positiveSum: number;
  mean: number | null;
  max: number | null;
  maxPartial: boolean;
  initialValue: number | null;
  partialStart: boolean;
  partialEnd: boolean;
  retentionClipped: boolean;
  coverageLabel: string | null;
}

export interface SessionSummary extends SessionStats, SessionMetadata {
  logicalSessionId: string;
  appRunId: string;
  runtimeSessionId: string;
  ordinal: number;
  instanceFqn: string;
  room: string | null;
  matrixType: MatrixType;
  logicalStartedAtUtcMs: number;
  firstObservedAtUtcMs: number;
  lastObservedAtUtcMs: number;
  initialKind: InitialKind;
}

export interface AgentDetailResponse extends ResponseHeader, Aggregate {
  agentKey: string;
  /** Count mode only; null in date mode. */
  scopeWindows: ScopeWindowCoverage[] | null;
  sessionsPage: SessionSummary[];
  nextSessionCursor: string | null;
}

export interface HistoryRecord {
  schemaVersion: number;
  recordId: string;
  appRunId: string;
  runtimeSessionId: string;
  logicalSessionId: string;
  sequence: number;
  ordinal: number;
  observedAtUtcMs: number;
  logicalStartedAtUtcMs: number | null;
  partialStart: boolean;
  partialEnd: boolean;
  initialRecordId: string | null;
  initialKind: InitialKind;
  continuityKnown: boolean;
  projectPath: string;
  projectName: string;
  agentKey: string;
  instanceFqn: string;
  room: string | null;
  matrixType: MatrixType;
  teamAttribution: TeamAttribution;
  teamIds: string[];
  codingAgentId: string | null;
  model: string | null;
  effort: string | null;
  metadataSource: MetadataSource;
  contextWindow: number | null;
  kind: RecordKind;
  percent: number | null;
  boundaryReason: BoundaryReason | null;
}

export interface ChangesPageResponse extends ResponseHeader {
  agentKey: string;
  logicalSessionId: string;
  recordsPage: HistoryRecord[];
  nextRecordCursor: string | null;
}

export type SidebarScopeKind = "known" | "ambiguous" | "unknown";

export interface SidebarScope {
  kind: SidebarScopeKind;
  agentKey: string | null;
  teamIds: string[];
}

/** Only a known scope carries windows; ambiguous/unknown are null, never zero. */
export interface SidebarEntry {
  runtimeSessionId: string;
  scope: SidebarScope;
  /** #4091 One bucket per configured count, in order. */
  sessionWindows: SessionWindowBucket[] | null;
}

export interface SidebarMeansResponse {
  schemaVersion: number;
  generatedAtUtcMs: number;
  entries: SidebarEntry[];
}

export type HistoryErrorCode =
  | "InvalidProject"
  | "InvalidQuery"
  | "InvalidCursor"
  | "UnsupportedVersion"
  | "StorageLimitMismatch"
  | "Busy"
  | "Timeout"
  | "Corrupt"
  | "ReadFailed"
  | "RecoveryRequired";

/** A refused request as the backend serializes it. */
export interface HistoryIpcError {
  code: HistoryErrorCode;
  message: string;
}

export function isHistoryIpcError(value: unknown): value is HistoryIpcError {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return typeof candidate.code === "string" && typeof candidate.message === "string";
}

/** #4091 "Últimas N sesiones" ("Última sesión" for 1), noting fewer available. */
export function sessionWindowLabel(requested: number, available?: number | null): string {
  const base = requested === 1 ? "Última sesión" : `Últimas ${requested} sesiones`;
  if (available === undefined || available === null || available >= requested) return base;
  return `${base} (${available} ${available === 1 ? "disponible" : "disponibles"})`;
}

/**
 * #4091 Resolves the colour window: `preferred` when it is a configured
 * count, otherwise the last configured count; plus its bucket, if served.
 */
export function selectSessionWindow(
  counts: readonly number[],
  preferred: number | null | undefined,
  buckets: readonly SessionWindowBucket[] | null | undefined,
): { requestedSessions: number; bucket: SessionWindowBucket | null } | null {
  const last = counts[counts.length - 1];
  if (last === undefined) return null;
  const isMember = preferred !== undefined && preferred !== null && counts.includes(preferred);
  const requestedSessions = isMember ? preferred : last;
  const bucket = buckets?.find((item) => item.requestedSessions === requestedSessions) ?? null;
  return { requestedSessions, bucket };
}

/** #4091 Backend records per session with one decimal; "—" when absent. */
export function formatRecordsPerSession(value: number | null | undefined): string {
  if (value === undefined || value === null) return "—";
  return value.toFixed(1);
}
