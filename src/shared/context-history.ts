// #3532 P06 - TypeScript mirror of the schema-version-2 context-history DTOs
// served by the four `get_context_history_*` commands. Field names, nulls and
// enum spellings follow the Rust serde output exactly; the frontend never
// recomputes a mean, a selection or a scope from these values.

export const CONTEXT_HISTORY_SCHEMA_VERSION = 2;
export const DAY_MS = 86_400_000;

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
  selection: "session_started";
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
  incomplete: boolean;
}

export interface AgentSummary extends Aggregate {
  agentKey: string;
  agentName: string;
  metadataBuckets: MetadataBucket[];
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

export interface SidebarBucket extends Aggregate {
  days: number;
  fromUtcMs: number;
  toUtcMs: number;
}

export interface SidebarWindows {
  day: SidebarBucket;
  week: SidebarBucket;
  month: SidebarBucket;
}

/** Only a known scope carries windows; ambiguous/unknown are null, never zero. */
export interface SidebarEntry {
  runtimeSessionId: string;
  scope: SidebarScope;
  windows: SidebarWindows | null;
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

/** Half-open UTC period `[to - days, to)` fixed at refresh time. */
export function periodEndingAt(toUtcMs: number, days: number): {
  fromUtcMs: number;
  toUtcMs: number;
} {
  return { fromUtcMs: toUtcMs - days * DAY_MS, toUtcMs };
}
