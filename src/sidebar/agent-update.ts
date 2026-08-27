import { createStore } from "solid-js/store";
import type { UnlistenFn } from "../shared/transport";
import {
  AgentUpdateAPI,
  onAgentInstallStateChanged,
  onAgentUpdateCommandFinished,
  onAgentUpdateCommandSkipped,
  onAgentUpdateCommandStarted,
  onAgentUpdatePrompt,
  onAgentUpdatePromptClosed,
  onAgentUpdatesFinished,
  onAgentUpdatesStarted,
} from "../shared/ipc";
import type {
  AgentUpdateCommandRef,
  AgentUpdateNode,
  AgentUpdatePrompt,
  AgentUpdateResult,
  AgentUpdateStatus,
  InstallState,
} from "../shared/types";
import { toastStore } from "../shared/stores/toasts";

/** #1327/#1551 sidebar state for the startup coding-agent update run. */
export interface AgentUpdateState {
  inProgress: boolean;
  prompt: AgentUpdatePrompt | null;
  /** #1551 - commands whose update sequence is running (events and snapshot, merged monotonically). */
  running: AgentUpdateCommandRef[];
  /** #1551 - per-command outcomes of this boot's pass (finished so far, then final). */
  results: AgentUpdateResult[];
  /** #1551 - `agent_updates_finished` was applied this boot; an older snapshot may then only merge results. */
  finishedSeen: boolean;
  /** #1551 - commands whose prompt was closed this boot (answered on any surface, or expired); a snapshot may never re-open one of them. */
  closedPrompts: string[];
  /** #1551 - the pass nodes in pass order (from agent_updates_started or the snapshot); only ever shrinks during a pass (skips). */
  nodes: AgentUpdateNode[];
  /** #1551 round 6 - commands whose node left this boot's pass (agent_update_command_skipped), recorded even when the node was never in the store; every later snapshot merge filters them out, exactly as closedPrompts protects prompts. Cleared by agent_updates_started. */
  skippedNodes: string[];
  /** #1551 - post-update install state per finished command, highest seq wins (from agent_install_state_changed). */
  installAfter: Record<string, InstallState>;
  /** #1551 - the persistent final view: shown from agent_updates_finished until the user closes it on this surface. */
  summary: "none" | "shown" | "dismissed";
}

export function agentUpdateInitialState(): AgentUpdateState {
  return {
    inProgress: false,
    prompt: null,
    running: [],
    results: [],
    finishedSeen: false,
    closedPrompts: [],
    nodes: [],
    skippedNodes: [],
    installAfter: {},
    summary: "none",
  };
}

export const agentUpdateStore = createStore<AgentUpdateState>(agentUpdateInitialState());

/**
 * #1327 - one sticky red toast per failed update command per instance. The event
 * and the snapshot can carry the same failure (the #609 subscribe-then-snapshot
 * race), so each command toasts at most once. #1551 - module-level so the
 * summary's close (`dismissAgentUpdateSummary`) shares the set with the wiring's
 * event and snapshot paths; `resetAgentUpdateForTests` clears it.
 */
const shownFailures = new Set<string>();

/** #1551 - full store reset plus the failure-toast dedup set (tests only). */
export function resetAgentUpdateForTests(): void {
  const [, setAgentUpdateState] = agentUpdateStore;
  setAgentUpdateState(agentUpdateInitialState());
  shownFailures.clear();
}

/**
 * #1327 - sticky red toast per failed update command, deduped through `shown`
 * (one toast per failed command per instance).
 */
export function showAgentUpdateFailures(
  results: AgentUpdateResult[],
  shown: Set<string>
): void {
  for (const result of results) {
    if (result.ok) continue;
    if (shown.has(result.command)) continue;
    shown.add(result.command);
    toastStore.error(
      `Auto-update failed for ${result.label} (${result.command}): ${result.error ?? "unknown error"}`,
      { durationMs: null }
    );
  }
}

/** #1551 - replace the result of `result.command`, else append. */
export function upsertResult(
  list: AgentUpdateResult[],
  result: AgentUpdateResult
): AgentUpdateResult[] {
  const index = list.findIndex((entry) => entry.command === result.command);
  if (index < 0) return [...list, result];
  return list.map((entry, i) => (i === index ? result : entry));
}

export function withoutRunning(
  list: AgentUpdateCommandRef[],
  command: string
): AgentUpdateCommandRef[] {
  return list.filter((ref) => ref.command !== command);
}

/** #1551 - `a` in order, then the entries of `b` whose command is absent from `a`. */
export function unionRunning(
  a: AgentUpdateCommandRef[],
  b: AgentUpdateCommandRef[]
): AgentUpdateCommandRef[] {
  const merged = [...a];
  for (const ref of b) {
    if (!merged.some((entry) => entry.command === ref.command)) merged.push(ref);
  }
  return merged;
}

export function addUnique(list: string[], value: string): string[] {
  return list.includes(value) ? list : [...list, value];
}

/** #1551 round 5 - the node of `command` leaves the pass. */
export function withoutNode(nodes: AgentUpdateNode[], command: string): AgentUpdateNode[] {
  return nodes.filter((node) => node.command !== command);
}

/**
 * #1551 round 5 - a node with that command exists: keep its position, label and
 * `updateCommands`, and take the incoming `installBefore` when present; else append.
 */
export function upsertNode(nodes: AgentUpdateNode[], node: AgentUpdateNode): AgentUpdateNode[] {
  const index = nodes.findIndex((entry) => entry.command === node.command);
  if (index < 0) return [...nodes, node];
  return nodes.map((entry, i) =>
    i === index && node.installBefore ? { ...entry, installBefore: node.installBefore } : entry
  );
}

/**
 * #1551 round 5/6 - `current` empty -> `incoming`; else `current` filtered to the
 * commands present in `incoming`, each taking the incoming `installBefore` when its
 * own is absent: nodes only shrink, so a snapshot can never resurrect a skipped
 * node, and a snapshot that predates a skip carries a superset the filter ignores.
 */
export function mergeNodes(current: AgentUpdateNode[], incoming: AgentUpdateNode[]): AgentUpdateNode[] {
  if (current.length === 0) return incoming;
  return current
    .filter((node) => incoming.some((entry) => entry.command === node.command))
    .map((node) => {
      if (node.installBefore) return node;
      const match = incoming.find((entry) => entry.command === node.command);
      return match?.installBefore ? { ...node, installBefore: match.installBefore } : node;
    });
}

/** #1551 - `true` iff `map[command]` is absent or has a lower `seq`. */
export function newerInstall(
  map: Record<string, InstallState>,
  command: string,
  install: InstallState
): boolean {
  const current = map[command];
  return !current || current.seq < install.seq;
}

/** #1551 - monotonic merge of a status snapshot into the store: events already applied are never
 *  downgraded by an older snapshot; a snapshot after `agent_updates_finished` may only add results;
 *  a snapshot's prompt is applied only when that prompt was not closed this boot. */
export function mergeSnapshot(current: AgentUpdateState, status: AgentUpdateStatus): AgentUpdateState {
  const results = status.results.reduce(upsertResult, current.results);
  if (current.finishedSeen) return { ...current, results };
  const running = unionRunning(current.running, status.running ?? []).filter((r) => !results.some((x) => x.command === r.command));
  const prompt = status.prompt && !current.closedPrompts.includes(status.prompt.command) ? status.prompt : current.prompt;
  const incomingNodes = status.nodes ?? [];
  // #1551 round 5 - a snapshot computed BEFORE the pass started (inProgress false, nodes []) can be delivered
  // AFTER agent_updates_started on Tauri (§3.3): it carries no node information and must never shrink the store.
  // Only a snapshot computed during the pass (inProgress true) can report a skip this surface missed.
  const mergedNodes = status.inProgress ? mergeNodes(current.nodes, incomingNodes) : (current.nodes.length > 0 ? current.nodes : incomingNodes);
  // #1551 round 6 - a skip this surface already saw wins over any snapshot, even one that seeds an empty store.
  const nodes = mergedNodes.filter((node) => !current.skippedNodes.includes(node.command));
  return { ...current, results, running, inProgress: current.inProgress || status.inProgress, prompt, nodes };
}

/** #1551 - the overlay's own answer closed this prompt (either outcome): protect it from an older snapshot. */
export function markPromptClosed(command: string): void {
  const [, setAgentUpdateState] = agentUpdateStore;
  setAgentUpdateState("closedPrompts", (list) => addUnique(list, command));
}

/**
 * #1551 round 5 - the user closed the final summary on THIS surface: hide it and
 * show the failure toasts the summary deferred (one per failed command, deduplicated
 * against the snapshot path by the same set). No IPC, no backend state.
 */
export function dismissAgentUpdateSummary(): void {
  const [state, setAgentUpdateState] = agentUpdateStore;
  if (state.summary !== "shown") return;
  setAgentUpdateState("summary", "dismissed");
  showAgentUpdateFailures(state.results, shownFailures);
}

/**
 * #1327 - wire the sidebar's startup coding-agent update notifications.
 * Subscribe-then-snapshot (#609): the listeners register BEFORE the snapshot is
 * queried, so a startup emit fired during mount is never dropped, and the
 * snapshot restores a prompt that was emitted pre-wiring. Returns the listener
 * unsubscribers for the caller's cleanup list. Extracted into its own module so
 * the wiring is unit-testable without rendering the whole sidebar.
 * #1551 - eight listeners (per-command events, skips, install states) and a
 * monotonic snapshot merge (`mergeSnapshot`).
 */
export async function wireAgentUpdateListeners(): Promise<UnlistenFn[]> {
  const unlisteners: UnlistenFn[] = [];
  const [state, setAgentUpdateState] = agentUpdateStore;

  unlisteners.push(
    await onAgentUpdatesStarted((payload) => {
      // #1551 - a pass starts clean and paints its nodes at once; `prompt` untouched.
      // An older backend's null payload leaves nodes empty: the timeline is then
      // built defensively from the per-command events (agent-update-status.ts).
      setAgentUpdateState({
        inProgress: true,
        running: [],
        results: [],
        finishedSeen: false,
        closedPrompts: [],
        nodes: payload?.nodes ?? [],
        skippedNodes: [],
        installAfter: {},
        summary: "none",
      });
    })
  );

  unlisteners.push(
    await onAgentUpdatePrompt((prompt) => {
      setAgentUpdateState("prompt", prompt);
    })
  );

  // F4: the prompt stopped being pending (answered on any surface, or timed out).
  // #1551 P7: a closure clears THIS surface's prompt only when it names the shown
  // prompt, so a delayed closure of A never erases a newer prompt B; the command is
  // always recorded so an older snapshot cannot resurrect it. An older backend's
  // null payload still clears unconditionally (it emits closures only from the
  // loop, before any next prompt).
  unlisteners.push(
    await onAgentUpdatePromptClosed((closed) => {
      const shown = state.prompt;
      if (!closed || shown?.command === closed.command) setAgentUpdateState("prompt", null);
      if (closed) setAgentUpdateState("closedPrompts", (list) => addUnique(list, closed.command));
    })
  );

  unlisteners.push(
    await onAgentUpdateCommandStarted((node) => {
      if (!state.running.some((ref) => ref.command === node.command)) {
        setAgentUpdateState("running", (list) => [
          ...list,
          { command: node.command, label: node.label },
        ]);
      }
      // Records installBefore; appends a node only when the store never received it
      // (lost `started` payload or older backend).
      setAgentUpdateState("nodes", (list) => upsertNode(list, node));
    })
  );

  unlisteners.push(
    await onAgentUpdateCommandSkipped((ref) => {
      // #1551 round 5/6 - the node leaves the timeline and the command is recorded
      // whether or not the node existed, so every later snapshot filters it.
      setAgentUpdateState({
        nodes: withoutNode(state.nodes, ref.command),
        skippedNodes: addUnique(state.skippedNodes, ref.command),
      });
    })
  );

  unlisteners.push(
    await onAgentInstallStateChanged(({ command, install }) => {
      // #1551 round 5/6 (FE N2) - accepted for any pass node, before or after its
      // result (after `started` no install event for a pass node can be pre-pass);
      // events for commands outside the pass belong to the Settings table.
      if (!state.nodes.some((node) => node.command === command)) return;
      if (!newerInstall(state.installAfter, command, install)) return;
      // Whole-map write on purpose: a path write (`set("installAfter", command, install)`)
      // would MERGE into an existing entry and keep keys the new state omits (a `missing`
      // after an `installed` would retain its stale `version`/`path`); the spread replaces it.
      setAgentUpdateState("installAfter", (map) => ({ ...map, [command]: install }));
    })
  );

  unlisteners.push(
    await onAgentUpdateCommandFinished((result) => {
      setAgentUpdateState({
        running: withoutRunning(state.running, result.command),
        results: upsertResult(state.results, result),
      });
    })
  );

  unlisteners.push(
    await onAgentUpdatesFinished(({ results }) => {
      // Authoritative. #1551 round 5 - a surface that showed the pass with at least
      // one result enters the persistent summary and defers its failure toasts to
      // the close; every other surface keeps the immediate toasts.
      const showSummary = state.inProgress && results.length > 0;
      setAgentUpdateState({
        inProgress: false,
        prompt: null,
        running: [],
        results,
        finishedSeen: true,
        summary: showSummary ? "shown" : state.summary,
      });
      if (!showSummary) showAgentUpdateFailures(results, shownFailures);
    })
  );

  // THEN snapshot (F8: a getStatus failure never breaks the live listeners).
  // #1551 round 6 (Grinch R3) - a snapshot received while the pass runs merges its
  // partial results and toasts NOTHING: that surface now shows the pass and its
  // toasts come at the summary close (or at `finished` when no summary enters).
  AgentUpdateAPI.getStatus()
    .then((status) => {
      if (!status) return;
      setAgentUpdateState(mergeSnapshot(state, status));
      if (!status.inProgress) showAgentUpdateFailures(status.results, shownFailures);
    })
    .catch((err) => {
      console.error("[agent-update] getStatus failed:", err);
    });

  return unlisteners;
}
