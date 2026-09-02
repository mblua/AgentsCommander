import type {
  AgentUpdateCommandRef,
  AgentUpdateNode,
  AgentUpdateOverviewRow,
  AgentUpdateResult,
  InstallState,
} from "../shared/types";

/**
 * #1551 - pure derivations for the Settings Auto-update table (plan 5.10) and the
 * Option 2 startup timeline (plan 5.10, round 5). Types only from `shared/types`;
 * this module never imports a store, so the overlay -> derivations -> types chain
 * stays acyclic (plan 11).
 */

export type ConfiguredState = "yes" | "no" | "ask";
export type InstalledState = "checking" | "installed" | "missing" | "probe-failed" | "unprobed";
export type LiveState = "idle" | "updating" | "ok" | "failed";

export interface AutoUpdateRowView {
  key: string;
  label: string;
  command: string;
  color: string;
  registered: boolean;
  configured: ConfiguredState;
  installed: { state: InstalledState; label: string; title?: string };
  live: { state: LiveState; label: string; title?: string };
}

/** round 5 (Option 2 timeline); #1691 adds the two nonterminal states and the cancelled terminal state. */
export type NodeState =
  | "pending"
  | "updating"
  | "verifying"
  | "cancelling"
  | "ok"
  | "failed"
  | "cancelled";

/**
 * #1691 - one timeline row. A row shows EXACTLY ONE line of text: a nonterminal row
 * shows `stateText` (one of the four fixed words), a terminal row shows `detail` (the
 * whole outcome as a single string). Never both, so no middle-dot split can reappear.
 */
export interface TimelineNodeView {
  command: string;
  label: string;
  updateCommands: string[];
  state: NodeState;
  /** the nonterminal word; `null` on a terminal row. */
  stateText: string | null;
  /** the terminal outcome string; `null` on a nonterminal row. */
  detail: string | null;
  detailTitle: string | null;
  /** #1691 - a terminal result was observed for this command (first winner). */
  terminal: boolean;
  /** #1691 - nonterminal and no cancellation requested yet: the row action is actionable. */
  cancellable: boolean;
}

export interface TimelineHeaderView {
  total: number;
  done: number;
  failed: number;
  percent: number;
  text: string;
}

export const CONFIGURED_LABELS = { yes: "Yes", no: "No", ask: "Will ask at startup" } as const;
export const LIVE_LABELS = { idle: "-", updating: "Updating...", ok: "Updated", failed: "Update failed" } as const;
/** #1691 - the only four nonterminal words. Terminal rows carry no state word at all. */
export const NODE_STATE_LABELS = {
  pending: "Pending",
  updating: "Updating...",
  verifying: "Verifying...",
  cancelling: "Cancelling...",
} as const;
export const UNKNOWN_ERROR_LABEL = "unknown error";
export const NOT_INSTALLED_LABEL = "Not installed";
/** #1691 - the exact terminal copy. ASCII `-` and `->` only: no Unicode arrow, no middle dot. */
export const NOTHING_TO_UPDATE_SUFFIX = "(Nothing to update)";
export const UPDATE_UNVERIFIED_LABEL = "Update completed - Version could not be verified";
export const CANCELLED_LABEL = "Cancelled";
export const FAILED_LABEL = "Failed";

/** `true` -> yes, `false` -> no, absent -> ask (the stored policy, registration aside). */
export function configuredState(map: Record<string, boolean>, command: string): ConfiguredState {
  const value = map[command];
  if (value === true) return "yes";
  if (value === false) return "no";
  return "ask";
}

export function installedView(install: InstallState): AutoUpdateRowView["installed"] {
  switch (install.status) {
    case "checking":
      return { state: "checking", label: "Checking..." };
    case "installed":
      // The backend always carries a version for `installed`; a version-less state
      // (unreachable) reads as presence only, never as an invented version.
      return { state: "installed", label: install.version ?? "Installed", title: install.path ?? undefined };
    case "missing":
      return { state: "missing", label: NOT_INSTALLED_LABEL, title: install.detail ?? undefined };
    case "probeFailed":
      return {
        state: "probe-failed",
        label: NOT_INSTALLED_LABEL,
        title: `Version check failed: ${install.detail ?? ""} (${install.path ?? ""})`,
      };
    case "unprobed":
      return { state: "unprobed", label: "Installed", title: `${install.detail ?? ""} (${install.path ?? ""})` };
  }
}

export function liveView(
  command: string,
  running: AgentUpdateCommandRef[],
  results: AgentUpdateResult[]
): AutoUpdateRowView["live"] {
  if (running.some((ref) => ref.command === command)) {
    return { state: "updating", label: LIVE_LABELS.updating };
  }
  const result = results.find((entry) => entry.command === command);
  if (!result) return { state: "idle", label: LIVE_LABELS.idle };
  if (result.ok) return { state: "ok", label: LIVE_LABELS.ok };
  return { state: "failed", label: LIVE_LABELS.failed, title: result.error ?? UNKNOWN_ERROR_LABEL };
}

/** Catalog order and duplicates preserved (one view per overview row). */
export function deriveAutoUpdateRows(
  rows: AgentUpdateOverviewRow[],
  input: {
    autoUpdateByCommand: Record<string, boolean>;
    registeredCommands: string[];
    running: AgentUpdateCommandRef[];
    results: AgentUpdateResult[];
  }
): AutoUpdateRowView[] {
  return rows.map((row) => ({
    key: row.key,
    label: row.label,
    command: row.command,
    color: row.color,
    registered: input.registeredCommands.includes(row.command),
    configured: configuredState(input.autoUpdateByCommand, row.command),
    installed: installedView(row.install),
    live: liveView(row.command, input.running, input.results),
  }));
}

/**
 * #1691 - the only version value a row may display: the version string of a nonempty
 * `installed` probe. Every other state (`missing`, `probeFailed`, `unprobed`, `checking`,
 * a version-less `installed`, no state at all) is not comparable and yields `null`, so a
 * row never prints an invented or non-comparable value.
 */
export function describeInstall(install: InstallState | null | undefined): string | null {
  if (!install) return null;
  if (install.status !== "installed") return null;
  return install.version ? install.version : null;
}

/**
 * #1691 - the exact single string of a terminal row, from that result's own fields only
 * (never from the one-shot install-cache events):
 *
 * - cancelled -> `Cancelled`;
 * - failed    -> `Failed - <reason>`, or `Failed` when a legacy result carries no reason;
 * - unchanged -> `<version> (Nothing to update)`;
 * - changed   -> `Ready - <old> -> <new>`;
 * - anything else succeeded (including a `changed`/`unchanged` claim whose versions are
 *   not both comparable) -> `Update completed - Version could not be verified`.
 */
export function outcomeText(result: AgentUpdateResult): string {
  if (result.outcome === "cancelled") return CANCELLED_LABEL;
  if (result.outcome === "failed") {
    const reason = result.error ?? "";
    return reason ? `${FAILED_LABEL} - ${reason}` : FAILED_LABEL;
  }
  const before = describeInstall(result.installBefore);
  const after = describeInstall(result.installAfter);
  if (result.change === "unchanged") {
    const version = after ?? before;
    return version ? `${version} ${NOTHING_TO_UPDATE_SUFFIX}` : UPDATE_UNVERIFIED_LABEL;
  }
  if (result.change === "changed" && before !== null && after !== null) {
    return `Ready - ${before} -> ${after}`;
  }
  return UPDATE_UNVERIFIED_LABEL;
}

/** #1691 - the terminal state of a result; `ok` keeps the pre-#1691 attribute value for `succeeded`. */
export function outcomeState(result: AgentUpdateResult): NodeState {
  if (result.outcome === "cancelled") return "cancelled";
  if (result.outcome === "failed") return "failed";
  return "ok";
}

/**
 * #1691 - first winner, then cancellation, then verification, then running. Cancellation
 * outranks verification so a verifying row that was already requested keeps saying
 * `Cancelling...`; both outrank `running` so a stale start never pulls a row backwards.
 */
function timelineNodeView(
  command: string,
  label: string,
  updateCommands: string[],
  running: AgentUpdateCommandRef[],
  verifying: AgentUpdateCommandRef[],
  results: AgentUpdateResult[],
  cancelling: ReadonlySet<string>
): TimelineNodeView {
  const result = results.find((entry) => entry.command === command);
  if (result) {
    const detail = outcomeText(result);
    return {
      command,
      label,
      updateCommands,
      state: outcomeState(result),
      stateText: null,
      detail,
      detailTitle: detail,
      terminal: true,
      cancellable: false,
    };
  }
  const state: NodeState = cancelling.has(command)
    ? "cancelling"
    : verifying.some((ref) => ref.command === command)
      ? "verifying"
      : running.some((ref) => ref.command === command)
        ? "updating"
        : "pending";
  return {
    command,
    label,
    updateCommands,
    state,
    stateText: NODE_STATE_LABELS[state],
    detail: null,
    detailTitle: null,
    terminal: false,
    cancellable: state !== "cancelling",
  };
}

/**
 * round 5 - one view per pass node, in node (pass) order; then, defensively (older
 * backend, lost `started` payload), every command present in `running`, `verifying` or
 * `results` but absent from `nodes`, in that order, with no update sequence. No command
 * yields two views.
 */
export function deriveTimelineNodes(
  nodes: AgentUpdateNode[],
  running: AgentUpdateCommandRef[],
  verifying: AgentUpdateCommandRef[],
  results: AgentUpdateResult[],
  cancelling: ReadonlySet<string> = new Set<string>()
): TimelineNodeView[] {
  const views: TimelineNodeView[] = [];
  const seen = new Set<string>();
  const push = (command: string, label: string, updateCommands: string[]) => {
    if (seen.has(command)) return;
    seen.add(command);
    views.push(
      timelineNodeView(command, label, updateCommands, running, verifying, results, cancelling)
    );
  };
  for (const node of nodes) push(node.command, node.label, node.updateCommands);
  for (const ref of running) push(ref.command, ref.label, []);
  for (const ref of verifying) push(ref.command, ref.label, []);
  for (const result of results) push(result.command, result.label, []);
  return views;
}

/**
 * #1691 - `<done> of <total> completed`, plus `, <n> failed` only when failures exist.
 * Every terminal row (cancelled included) counts as done; only `failed` counts as failed.
 */
export function deriveTimelineHeader(views: TimelineNodeView[]): TimelineHeaderView {
  const total = views.length;
  const done = views.filter((view) => view.terminal).length;
  const failed = views.filter((view) => view.state === "failed").length;
  const percent = total ? Math.round((done / total) * 100) : 0;
  let text = `${done} of ${total} completed`;
  if (failed > 0) text += `, ${failed} failed`;
  return { total, done, failed, percent, text };
}
