import type {
  BlockerProcess,
  BlockerReport,
  BlockerSession,
  DiagnosticError,
  IgnoredSessionRecord,
} from "../../shared/types";

export type NormalizedBlockerReport = {
  liveSessions: BlockerSession[];
  externalProcesses: BlockerProcess[];
  ignoredExited: IgnoredSessionRecord[];
  rawDeleteError: string;
  restartManagerError?: DiagnosticError;
  restartManagerAvailable: boolean;
};

export function normalizeBlockerReport(report: BlockerReport): NormalizedBlockerReport {
  return {
    liveSessions: report.liveSessions ?? report.sessions ?? [],
    externalProcesses: report.externalProcesses ?? report.processes ?? [],
    ignoredExited: report.exitedSessionRecordsIgnored ?? [],
    rawDeleteError: report.rawDeleteError ?? report.rawOsError,
    restartManagerError: report.restartManagerError,
    restartManagerAvailable: report.restartManagerAvailable ?? report.diagnosticAvailable,
  };
}

export function workgroupDeleteDiagnosticCase(
  report: BlockerReport,
):
  | "live-sessions"
  | "external-processes"
  | "ignored-exited-only"
  | "unknown-lock"
  | "non-windows-unavailable" {
  const normalized = normalizeBlockerReport(report);
  if (normalized.liveSessions.length > 0) return "live-sessions";
  if (normalized.externalProcesses.length > 0) return "external-processes";
  if (normalized.ignoredExited.length > 0) return "ignored-exited-only";
  if (!report.diagnosticAvailable && report.platform !== "windows") {
    return "non-windows-unavailable";
  }
  return "unknown-lock";
}
