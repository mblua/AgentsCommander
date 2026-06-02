import { describe, expect, it } from "vitest";
import type { BlockerReport } from "../../shared/types";
import {
  normalizeBlockerReport,
  workgroupDeleteDiagnosticCase,
} from "./workgroup-delete-diagnostics";

const baseReport = (): BlockerReport => ({
  workgroup: "wg-7-dev-team",
  platform: "windows",
  diagnosticAvailable: true,
  rawOsError: "sharing violation",
  sessions: [],
  processes: [],
});

describe("workgroup delete diagnostics", () => {
  it("selects live sessions when authoritative live session fields are present", () => {
    const report: BlockerReport = {
      ...baseReport(),
      liveSessions: [
        {
          sessionId: "session-1",
          agentName: "agentscommander:wg-7-dev-team/dev-rust",
          cwd: "C:/work/wg-7-dev-team/__agent_dev-rust",
        },
      ],
    };

    expect(workgroupDeleteDiagnosticCase(report)).toBe("live-sessions");
  });

  it("selects external processes when no live sessions exist", () => {
    const report: BlockerReport = {
      ...baseReport(),
      externalProcesses: [
        {
          pid: 42,
          name: "powershell.exe",
          files: [],
        },
      ],
    };

    expect(workgroupDeleteDiagnosticCase(report)).toBe("external-processes");
  });

  it("selects ignored exited records when they are the only identified records", () => {
    const report: BlockerReport = {
      ...baseReport(),
      exitedSessionRecordsIgnored: [
        {
          sessionId: "session-2",
          agentName: "agentscommander:wg-7-dev-team/architect",
          cwd: "C:/work/wg-7-dev-team/__agent_architect",
          status: "Exited(0)",
          waitingForInput: false,
        },
      ],
    };

    expect(workgroupDeleteDiagnosticCase(report)).toBe("ignored-exited-only");
  });

  it("selects unknown lock when Windows has no identified blockers", () => {
    const report: BlockerReport = {
      ...baseReport(),
      diagnosticAvailable: false,
      restartManagerError: {
        message: "RmStartSession failed: WIN32_ERROR=5 (Access denied)",
        code: 5,
        meaning: "Access denied",
      },
    };

    expect(workgroupDeleteDiagnosticCase(report)).toBe("unknown-lock");
  });

  it("selects non-Windows unavailable when diagnostics are unavailable off Windows", () => {
    const report: BlockerReport = {
      ...baseReport(),
      platform: "linux",
      diagnosticAvailable: false,
    };

    expect(workgroupDeleteDiagnosticCase(report)).toBe("non-windows-unavailable");
  });

  it("normalizes legacy blocker payload fields", () => {
    const report: BlockerReport = {
      ...baseReport(),
      rawOsError: "legacy raw error",
      sessions: [
        {
          sessionId: "session-legacy",
          agentName: "agentscommander:wg-7-dev-team/dev-webpage-ui",
          cwd: "C:/work/wg-7-dev-team/__agent_dev-webpage-ui",
        },
      ],
      processes: [
        {
          pid: 123,
          name: "node.exe",
          files: ["C:/work/wg-7-dev-team/repo-AgentsCommander/package.json"],
        },
      ],
    };

    const normalized = normalizeBlockerReport(report);
    expect(normalized.liveSessions).toBe(report.sessions);
    expect(normalized.externalProcesses).toBe(report.processes);
    expect(normalized.rawDeleteError).toBe("legacy raw error");
    expect(normalized.ignoredExited).toEqual([]);
  });
});
