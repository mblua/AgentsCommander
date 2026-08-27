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

/** round 5 (Option 2 timeline) */
export type NodeState = "pending" | "updating" | "ok" | "failed";

export interface TimelineNodeView {
  command: string;
  label: string;
  updateCommands: string[];
  state: NodeState;
  stateText: string;
  detail: string | null;
  detailTitle: string | null;
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
export const NODE_STATE_LABELS = {
  pending: "Pendiente",
  updating: "Actualizando...",
  ok: "Listo",
  failed: "Falló",
} as const;
export const VERSION_MISSING_LABEL = "no instalada";
export const VERSION_UNDETECTED_LABEL = "versión no detectada";
export const UNKNOWN_ERROR_LABEL = "unknown error";
export const NOT_INSTALLED_LABEL = "Not installed";

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
 * round 5 - what a probe result says about the installed version: the version string,
 * `no instalada`, `versión no detectada`, or `null` when there is no claim to make
 * (`unprobed`, `checking`, a version-less `installed`, no state at all).
 */
export function describeInstall(install: InstallState | null | undefined): string | null {
  if (!install) return null;
  switch (install.status) {
    case "installed":
      return install.version ? install.version : null;
    case "missing":
      return VERSION_MISSING_LABEL;
    case "probeFailed":
      return VERSION_UNDETECTED_LABEL;
    default:
      return null;
  }
}

/**
 * round 5 - `<before> → <after>` when both were read, the honest partial form when only
 * one was, `null` when neither. Never an invented value.
 */
export function versionTransitionText(
  before: InstallState | null | undefined,
  after: InstallState | null | undefined
): string | null {
  const b = describeInstall(before);
  const a = describeInstall(after);
  if (b !== null && a !== null) return `${b} → ${a}`;
  if (b !== null) return b;
  if (a !== null) return a;
  return null;
}

function timelineNodeView(
  command: string,
  label: string,
  updateCommands: string[],
  installBefore: InstallState | null | undefined,
  running: AgentUpdateCommandRef[],
  results: AgentUpdateResult[],
  installAfter: Record<string, InstallState>
): TimelineNodeView {
  const result = results.find((entry) => entry.command === command);
  const state: NodeState = result
    ? result.ok
      ? "ok"
      : "failed"
    : running.some((ref) => ref.command === command)
      ? "updating"
      : "pending";
  const transition = versionTransitionText(installBefore, installAfter[command]);
  let detail: string | null = null;
  if (state === "ok") {
    detail = transition;
  } else if (state === "failed") {
    detail = [result?.error ?? UNKNOWN_ERROR_LABEL, transition].filter(Boolean).join(" · ");
  }
  return {
    command,
    label,
    updateCommands,
    state,
    stateText: NODE_STATE_LABELS[state],
    detail,
    detailTitle: detail,
  };
}

/**
 * round 5 - one view per pass node, in node (pass) order; then, defensively (older
 * backend, lost `started` payload), every command present in `running` or `results`
 * but absent from `nodes`, in `running` order then `results` order, with no update
 * sequence. No command yields two views.
 */
export function deriveTimelineNodes(
  nodes: AgentUpdateNode[],
  running: AgentUpdateCommandRef[],
  results: AgentUpdateResult[],
  installAfter: Record<string, InstallState>
): TimelineNodeView[] {
  const views: TimelineNodeView[] = [];
  const seen = new Set<string>();
  for (const node of nodes) {
    if (seen.has(node.command)) continue;
    seen.add(node.command);
    views.push(
      timelineNodeView(
        node.command,
        node.label,
        node.updateCommands,
        node.installBefore,
        running,
        results,
        installAfter
      )
    );
  }
  for (const ref of running) {
    if (seen.has(ref.command)) continue;
    seen.add(ref.command);
    views.push(timelineNodeView(ref.command, ref.label, [], null, running, results, installAfter));
  }
  for (const result of results) {
    if (seen.has(result.command)) continue;
    seen.add(result.command);
    views.push(timelineNodeView(result.command, result.label, [], null, running, results, installAfter));
  }
  return views;
}

/** round 5 - `n de N completados` plus ` · k falló` / ` · k fallaron`, and the bar's percent. */
export function deriveTimelineHeader(views: TimelineNodeView[]): TimelineHeaderView {
  const total = views.length;
  const done = views.filter((view) => view.state === "ok" || view.state === "failed").length;
  const failed = views.filter((view) => view.state === "failed").length;
  const percent = total ? Math.round((done / total) * 100) : 0;
  let text = `${done} de ${total} completados`;
  if (failed === 1) text += ` · ${failed} falló`;
  else if (failed > 1) text += ` · ${failed} fallaron`;
  return { total, done, failed, percent, text };
}
