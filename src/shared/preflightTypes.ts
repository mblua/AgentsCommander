/** #3516 - wire mirror of the P04 preflight test API (camelCase serde). */
export type PreflightStageSource = "codingAgent" | "profile";

export type PreflightStageStatus =
  | "pending" | "running" | "passed" | "failed" | "timedOut"
  | "spawnFailed" | "signaled" | "cancelled" | "notRun";

export interface PreflightStageReport {
  source: PreflightStageSource;
  label: string;
  effectiveProfile: string;
  commandText: string;
  timeoutSeconds: number;
  status: PreflightStageStatus;
  exitCode: number | null;
  durationMs: number | null;
  stdout: string;
  stderr: string;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  detail: string | null;
  cleanupDefect: string | null;
}

export interface PreflightTargetDescription {
  agentId: string;
  replicaPath: string;
  cwd: string;
  requestedProfile: string;
  effectiveProfile: string;
  fallbackApplied: boolean;
  stopped: boolean;
  stages: {
    source: PreflightStageSource;
    label: string;
    commandText: string;
    timeoutSeconds: number;
  }[];
  settingsRevision: string;
  activeRunId: string | null;
  unavailableReason: string | null;
}

export interface PreflightTestSnapshot {
  runId: string;
  agentId: string;
  replicaPath: string;
  effectiveProfile: string;
  settingsRevision: string;
  phase: "preparing" | "running" | "finished";
  outcome: "passed" | "failed" | "cancelled" | null;
  message: string | null;
  stages: PreflightStageReport[];
}

/** #3832 - one unsaved preflight stage; null command is empty, null timeout is 60. */
export interface PreflightDraftStage {
  command: string | null;
  timeoutSeconds: number | null;
}

/** #3832 - unsaved preflight commands and timeouts; Profile keys are letters A-Z. */
export interface PreflightTestDraft {
  codingAgent: PreflightDraftStage;
  profiles: Record<string, PreflightDraftStage>;
}

/** #3832 - one replica row returned by list_preflight_test_targets. */
export interface PreflightTestTarget {
  replicaPath: string;
  requestedProfile: string;
  effectiveProfile: string;
  fallbackApplied: boolean;
  running: boolean;
}
