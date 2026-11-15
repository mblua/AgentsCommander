// #3516 - pure preflight settings and test-panel logic. No Solid, no IPC.
import type {
  AppSettings,
  PreflightStageReport,
  PreflightStageSource,
  PreflightTestSnapshot,
} from "../../../shared/types";
import type {
  PreflightDraftStage, PreflightTestDraft, PreflightTestTarget,
} from "../../../shared/preflightTypes";

const MAX_TIMEOUT_SECONDS = 4294967295;

export const INVALID_TIMEOUT_TEXT = "Enter a whole number of seconds from 1 to 4294967295.";
export const TIMEOUT_SAVE_ERROR = `Preflight timeout: ${INVALID_TIMEOUT_TEXT}`;
export const CANCELLING_TEXT = "Cancelling…";
const PREPARING_TEXT = "Preparing test context…";

export type TimeoutParse = { ok: true; value: number | undefined } | { ok: false };

/** Empty means "use the default" (the key is removed); else a whole number 1..u32::MAX. */
export function parseTimeoutInput(text: string): TimeoutParse {
  const trimmed = text.trim();
  if (trimmed === "") return { ok: true, value: undefined };
  if (!/^\d+$/.test(trimmed)) return { ok: false };
  const value = Number(trimmed);
  if (value < 1 || value > MAX_TIMEOUT_SECONDS) return { ok: false };
  return { ok: true, value };
}

/** Key of one invalid timeout draft: the agent row or one profile cell. */
export function timeoutKey(agentId: string, letter: string | null): string {
  return letter === null ? `agent:${agentId}` : `cell:${agentId}:${letter}`;
}

/** True when `key` is the agent's own timeout key or one of its cell keys. */
export function isTimeoutKeyOfAgent(key: string, agentId: string): boolean {
  return key === timeoutKey(agentId, null) || key.startsWith(`cell:${agentId}:`);
}

/** The same saved inputs as the P04 revision, so an unsaved draft is detectable. */
export function preflightFingerprint(settings: AppSettings, agentId: string): string {
  const row = settings.agents.find((agent) => agent.id === agentId);
  const cells = settings.codingAgentProfiles?.profilesByAgent?.[agentId] ?? {};
  const cellInputs = Object.entries(cells).sort(([a], [b]) => a.localeCompare(b)).map(([letter, cell]) => {
    return [letter, {
      command: cell.command,
      env: cell.env,
      enabled: cell.enabled,
      preflight: cell.preflight,
      preflightTimeoutSeconds: cell.preflightTimeoutSeconds,
    }];
  });
  return JSON.stringify({
    agent: row ? {
      command: row.command,
      envs: row.envs,
      preflight: row.preflight,
      preflightTimeoutSeconds: row.preflightTimeoutSeconds,
      configSeed: row.configSeed,
    } : null,
    cells: cellInputs,
    defaultShell: settings.defaultShell,
    defaultShellArgs: settings.defaultShellArgs,
  });
}

/** `editedSinceResult` is sticky: the caller resets it only when a new run starts. */
export function isStale(
  result: { settingsRevision: string },
  currentRevision: string,
  editedSinceResult: boolean,
): boolean {
  return result.settingsRevision !== currentRevision || editedSinceResult;
}

/** Display lines for one stage: its status, then any detail or cleanup line. */
export function stageStatusText(report: PreflightStageReport): string[] {
  const lines = [statusLine(report)];
  if (report.status === "spawnFailed" && report.detail) lines.push(report.detail);
  if (report.cleanupDefect) lines.push(`Cleanup reported an error: ${report.cleanupDefect}`);
  return lines;
}

function statusLine(report: PreflightStageReport): string {
  switch (report.status) {
    case "pending": return "Waiting";
    case "running": return "Running…";
    case "passed": return "Passed — exit code 0";
    case "failed":
      return report.exitCode === null ? "Failed" : `Failed — exit code ${report.exitCode}`;
    case "spawnFailed": return "Command could not be started";
    case "timedOut": return "Timed out";
    case "signaled": return "Terminated by a signal";
    case "cancelled": return "Cancelled";
    case "notRun": return "Not run — previous stage failed";
  }
}

function runningLine(source: PreflightStageSource): string {
  return source === "codingAgent"
    ? "Running Coding Agent preflight…"
    : "Running Profile preflight…";
}

/** The progress line of an unfinished run; `null` once finished (the banner shows). */
export function activeLine(snapshot: PreflightTestSnapshot, cancelSent: boolean): string | null {
  if (cancelSent || snapshot.message === CANCELLING_TEXT) return CANCELLING_TEXT;
  if (snapshot.phase === "preparing") return PREPARING_TEXT;
  const running = snapshot.stages.find((stage) => stage.status === "running");
  if (running) return runningLine(running.source);
  if (snapshot.phase === "running") {
    const started = snapshot.stages.filter((stage) => stage.status !== "pending");
    const last = started[started.length - 1];
    return last ? runningLine(last.source) : PREPARING_TEXT;
  }
  return null;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

export const NO_TARGETS_TEXT = "No compatible stopped agent is available. Create or stop an "
  + "agent before testing.";
export const ALL_RUNNING_TEXT = "Stop one of these agents before testing.";

type DraftSource = { preflight?: string; preflightTimeoutSeconds?: number } | undefined;

/** #3832 - one stage of the draft payload: only the command and its timeout. */
function draftStage(source: DraftSource): PreflightDraftStage {
  return {
    command: source?.preflight ?? null,
    timeoutSeconds: source?.preflightTimeoutSeconds ?? null,
  };
}

/** #3832 - the unsaved preflight commands and timeouts of one Coding Agent. */
export function buildPreflightDraft(settings: AppSettings, agentId: string): PreflightTestDraft {
  const row = settings.agents.find((agent) => agent.id === agentId);
  const cells = settings.codingAgentProfiles?.profilesByAgent?.[agentId] ?? {};
  const profiles: Record<string, PreflightDraftStage> = {};
  for (const letter of Object.keys(cells).sort()) profiles[letter] = draftStage(cells[letter]);
  return { codingAgent: draftStage(row), profiles };
}

/** #3832 - stable text of a draft payload, to match a description to its request. */
export function draftKey(draft: PreflightTestDraft): string {
  return JSON.stringify(draft);
}

/** #3832 - Profile-cell mode keeps rows that launch with `letter`; card mode keeps all. */
export function rowsForLetter(
  rows: readonly PreflightTestTarget[], letter: string | null,
): PreflightTestTarget[] {
  return letter === null ? [...rows] : rows.filter((row) => row.effectiveProfile === letter);
}

/** #3832 - why no row can be chosen (empty or all running), or null when one can. */
export function targetListText(
  rows: readonly PreflightTestTarget[], letter: string | null,
): string | null {
  if (rows.length === 0) {
    return letter === null
      ? NO_TARGETS_TEXT
      : `No agent launches with Profile ${letter}. Assign Profile ${letter} to an agent `
        + "to test it here.";
  }
  if (rows.every((row) => row.running)) return ALL_RUNNING_TEXT;
  return null;
}
