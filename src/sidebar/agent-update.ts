import { createStore } from "solid-js/store";
import type { UnlistenFn } from "../shared/transport";
import {
  AgentUpdateAPI,
  onAgentInstallStateChanged,
  onAgentUpdateCancellationChanged,
  onAgentUpdateCommandFinished,
  onAgentUpdateCommandSkipped,
  onAgentUpdateCommandStarted,
  onAgentUpdateCommandVerifying,
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
  AgentUpdateResultWire,
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
  /** #1691 - commands whose sequence ended and whose post-update probe is running, in pass order.
   *  Unfinished: done=false, failed=false, and still cancellable until a terminal result lands. */
  verifying: AgentUpdateCommandRef[];
  /** #1691 - commands whose cancellation the backend accepted, in pass order (authoritative). */
  cancelRequested: AgentUpdateCommandRef[];
  /** #1691 - a batch cancellation was accepted for this pass. Monotonic within a pass: only
   *  `agent_updates_started` clears it, so no delayed `false` can re-enable the controls. */
  cancelAllRequested: boolean;
  /** #1691 - FRONTEND ONLY, pass-local: commands whose cancel invoke got a backend response before
   *  authoritative hydration converged. It keeps that row disabled across a missing/delayed event,
   *  a failed refresh and a component remount. Never sent to, or read from, the backend. */
  cancelResponses: string[];
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
    verifying: [],
    cancelRequested: [],
    cancelAllRequested: false,
    cancelResponses: [],
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

/** #1691 - the only two visible cancellation-failure strings, reserved for an invoke/backend
 *  rejection that arrived BEFORE any response. A post-response hydration failure has no copy. */
export const ROW_CANCEL_FAILED_TOAST = "Could not cancel the coding agent update.";
export const BATCH_CANCEL_FAILED_TOAST = "Could not cancel coding agent updates.";

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
 * #1691 - the canonical result shape from any backend. An older backend omits the four
 * #1691 keys; this is the ONLY tolerated inference and it is exact:
 * missing `outcome` -> `ok ? "succeeded" : "failed"`; missing installs -> `null`;
 * missing `change` -> `"unknown"`; missing verification diagnostic -> `undefined`.
 * Every fold and every notification classification runs on the normalized result.
 */
export function normalizeAgentUpdateResult(result: AgentUpdateResultWire): AgentUpdateResult {
  return {
    ...result,
    outcome: result.outcome ?? (result.ok ? "succeeded" : "failed"),
    installBefore: result.installBefore ?? null,
    installAfter: result.installAfter ?? null,
    change: result.change ?? "unknown",
  };
}

export function normalizeAgentUpdateResults(results: AgentUpdateResultWire[]): AgentUpdateResult[] {
  return results.map(normalizeAgentUpdateResult);
}

/**
 * #1327 - sticky red toast per failed update command, deduped through `shown`
 * (one toast per failed command per instance).
 * #1691 - `outcome === "failed"` is the ONLY failure predicate: a cancelled result has
 * `ok === false` and an absent probe is not a failure, so neither may toast.
 */
export function showAgentUpdateFailures(
  results: AgentUpdateResult[],
  shown: Set<string>
): void {
  for (const result of results) {
    if (result.outcome !== "failed") continue;
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

/** #1691 - a terminal result was already observed for `command`: it is the first winner. */
export function hasResult(list: AgentUpdateResult[], command: string): boolean {
  return list.some((entry) => entry.command === command);
}

export function withoutRunning(
  list: AgentUpdateCommandRef[],
  command: string
): AgentUpdateCommandRef[] {
  return list.filter((ref) => ref.command !== command);
}

/** #1691 - drop every ref whose command already has a terminal result. */
export function withoutTerminal(
  list: AgentUpdateCommandRef[],
  results: AgentUpdateResult[]
): AgentUpdateCommandRef[] {
  return list.filter((ref) => !hasResult(results, ref.command));
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

/** #1691 - the label the pass knows for `command`, from any surface that carries one. */
export function labelFor(state: AgentUpdateState, command: string): string {
  const node = state.nodes.find((entry) => entry.command === command);
  if (node) return node.label;
  const ref =
    state.running.find((entry) => entry.command === command) ??
    state.verifying.find((entry) => entry.command === command) ??
    state.cancelRequested.find((entry) => entry.command === command);
  if (ref) return ref.label;
  return state.results.find((entry) => entry.command === command)?.label ?? command;
}

/** #1551 - monotonic merge of a status snapshot into the store: events already applied are never
 *  downgraded by an older snapshot; a snapshot after `agent_updates_finished` may only add results;
 *  a snapshot's prompt is applied only when that prompt was not closed this boot.
 *  #1691 - results are normalized first, so every fold below classifies on the canonical shape;
 *  running/verifying/cancel arrays drop terminal rows; `cancelAllRequested` only ever ORs upward. */
export function mergeSnapshot(current: AgentUpdateState, status: AgentUpdateStatus): AgentUpdateState {
  const results = normalizeAgentUpdateResults(status.results).reduce(upsertResult, current.results);
  const cancelResponses = current.cancelResponses.filter((command) => !hasResult(results, command));
  // #1691 - the batch latch is monotonic even across the finished boundary: a delayed `false`
  // must never re-enable a control the response already latched.
  const cancelAllRequested = current.cancelAllRequested || (status.cancelAllRequested ?? false);
  if (current.finishedSeen) {
    return {
      ...current,
      results,
      cancelResponses,
      cancelAllRequested,
      verifying: withoutTerminal(current.verifying, results),
      cancelRequested: withoutTerminal(current.cancelRequested, results),
    };
  }
  const running = withoutTerminal(unionRunning(current.running, status.running ?? []), results);
  const verifying = withoutTerminal(unionRunning(current.verifying, status.verifying ?? []), results);
  // #1691 - a row that verifies is no longer running, whichever source reported it first.
  const runningWithoutVerifying = running.filter(
    (ref) => !verifying.some((entry) => entry.command === ref.command)
  );
  const cancelRequested = withoutTerminal(
    unionRunning(current.cancelRequested, status.cancelRequested ?? []),
    results
  );
  const prompt = status.prompt && !current.closedPrompts.includes(status.prompt.command) ? status.prompt : current.prompt;
  const incomingNodes = status.nodes ?? [];
  // #1551 round 5 - a snapshot computed BEFORE the pass started (inProgress false, nodes []) can be delivered
  // AFTER agent_updates_started on Tauri (§3.3): it carries no node information and must never shrink the store.
  // Only a snapshot computed during the pass (inProgress true) can report a skip this surface missed.
  const mergedNodes = status.inProgress ? mergeNodes(current.nodes, incomingNodes) : (current.nodes.length > 0 ? current.nodes : incomingNodes);
  // #1551 round 6 - a skip this surface already saw wins over any snapshot, even one that seeds an empty store.
  const nodes = mergedNodes.filter((node) => !current.skippedNodes.includes(node.command));
  return {
    ...current,
    results,
    running: runningWithoutVerifying,
    verifying,
    cancelRequested,
    cancelAllRequested,
    cancelResponses,
    inProgress: current.inProgress || status.inProgress,
    prompt,
    nodes,
  };
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
 * #1691 - the one post-response reconciliation read. It collects a terminal race the
 * response could not carry. A rejection here is NOT a cancellation failure: the response
 * already latched the store, so the diagnostic goes to `console.error` only, nothing is
 * un-latched, no toast is shown, and no cancellation is ever reissued because of it. A
 * later event, or a listener-first/remount hydration, performs the reconciliation.
 */
async function hydrateAfterCancelResponse(): Promise<void> {
  const [state, setAgentUpdateState] = agentUpdateStore;
  try {
    const status = await AgentUpdateAPI.getStatus();
    if (status) setAgentUpdateState(mergeSnapshot(state, status));
  } catch (err) {
    console.error("[agent-update] getStatus after cancel failed:", err);
  }
}

/**
 * #1691 - cancel ONE command. Returns `true` when a backend response was received (the
 * store latched the row and the caller must not retry), `false` only when the invoke or
 * the backend rejected BEFORE any response (nothing latched; the row may be retried).
 *
 * Every disposition latches the row: `requested`/`already_requested` also fold it into the
 * authoritative `cancelRequested`, while `already_terminal`/`not_in_pass` fabricate no
 * result and keep only the latch until an authoritative merge or a new pass clears it.
 */
export async function cancelAgentUpdateRow(command: string): Promise<boolean> {
  const [state, setAgentUpdateState] = agentUpdateStore;
  const label = labelFor(state, command);
  let disposition: string;
  try {
    disposition = (await AgentUpdateAPI.cancel(command)).disposition;
  } catch (err) {
    console.error("[agent-update] agent_update_cancel failed:", err);
    toastStore.error(ROW_CANCEL_FAILED_TOAST, { durationMs: null });
    return false;
  }
  const folded = disposition === "requested" || disposition === "already_requested";
  setAgentUpdateState({
    cancelResponses: addUnique(state.cancelResponses, command),
    cancelRequested: folded
      ? withoutTerminal(unionRunning(state.cancelRequested, [{ command, label }]), state.results)
      : state.cancelRequested,
  });
  await hydrateAfterCancelResponse();
  return true;
}

/**
 * #1691 - cancel the whole pass. Returns `true` when a backend response was received:
 * `cancelAllRequested` is then latched BEFORE the caller clears its local pending, so a
 * missing or delayed event can never re-enable the controls. `false` only when the invoke
 * or the backend rejected before any response.
 */
export async function cancelAllAgentUpdates(): Promise<boolean> {
  const [state, setAgentUpdateState] = agentUpdateStore;
  let requested: AgentUpdateCommandRef[];
  let alreadyRequested: AgentUpdateCommandRef[];
  try {
    const response = await AgentUpdateAPI.cancelAll();
    requested = response.requested;
    alreadyRequested = response.alreadyRequested;
  } catch (err) {
    console.error("[agent-update] agent_updates_cancel_all failed:", err);
    toastStore.error(BATCH_CANCEL_FAILED_TOAST, { durationMs: null });
    return false;
  }
  // `alreadyTerminal` is deliberately NOT folded: it fabricates no result and no request.
  const folded = [...requested, ...alreadyRequested];
  setAgentUpdateState({
    cancelAllRequested: true,
    cancelRequested: withoutTerminal(unionRunning(state.cancelRequested, folded), state.results),
    cancelResponses: folded.reduce(
      (list, ref) => addUnique(list, ref.command),
      state.cancelResponses
    ),
  });
  await hydrateAfterCancelResponse();
  return true;
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
 * #1691 - ten listeners (verification and cancellation). Cleanup only unlistens: it
 * never invokes a cancel API, so unmounting a surface never cancels a pass.
 */
export async function wireAgentUpdateListeners(): Promise<UnlistenFn[]> {
  const unlisteners: UnlistenFn[] = [];
  const [state, setAgentUpdateState] = agentUpdateStore;

  unlisteners.push(
    await onAgentUpdatesStarted((payload) => {
      // #1551 - a pass starts clean and paints its nodes at once; `prompt` untouched.
      // An older backend's null payload leaves nodes empty: the timeline is then
      // built defensively from the per-command events (agent-update-status.ts).
      // #1691 - the verification/cancellation collections AND both response latches are
      // reset here, and only here: a new pass is the single point where a latched
      // control becomes actionable again.
      setAgentUpdateState({
        inProgress: true,
        running: [],
        verifying: [],
        cancelRequested: [],
        cancelAllRequested: false,
        cancelResponses: [],
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
      // #1691 - a start is ignored for a terminal row (first winner) and for a verifying
      // row (which already moved past running), so neither can be pulled backwards.
      if (hasResult(state.results, node.command)) return;
      if (state.verifying.some((ref) => ref.command === node.command)) return;
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
    await onAgentUpdateCommandVerifying((ref) => {
      // #1691 - ignored for a terminal row; moves a nonterminal row out of running; and
      // never touches `cancelRequested`, so an already-observed request is never cleared.
      if (hasResult(state.results, ref.command)) return;
      setAgentUpdateState({
        running: withoutRunning(state.running, ref.command),
        verifying: unionRunning(state.verifying, [ref]),
      });
    })
  );

  unlisteners.push(
    await onAgentUpdateCancellationChanged((payload) => {
      // #1691 - the payload is a FULL snapshot, but the fold is a union filtered of terminal
      // rows: a delayed snapshot can neither resurrect a terminal row nor drop a request this
      // surface already observed. `cancelAllRequested` only ever ORs upward.
      setAgentUpdateState({
        cancelRequested: withoutTerminal(
          unionRunning(state.cancelRequested, payload.cancelRequested ?? []),
          state.results
        ),
        cancelAllRequested: state.cancelAllRequested || payload.cancelAllRequested,
      });
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
      // #1691 - this cache never feeds a terminal row's text: an outcome is rendered
      // from that result's own `installBefore`/`installAfter`/`change` only.
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
      // #1691 - terminal FIRST WINNER: a second result for the same command is dropped,
      // and the row leaves every in-progress collection and both response latches.
      if (hasResult(state.results, result.command)) return;
      const normalized = normalizeAgentUpdateResult(result);
      setAgentUpdateState({
        running: withoutRunning(state.running, result.command),
        verifying: withoutRunning(state.verifying, result.command),
        cancelRequested: withoutRunning(state.cancelRequested, result.command),
        cancelResponses: state.cancelResponses.filter((command) => command !== result.command),
        results: upsertResult(state.results, normalized),
      });
    })
  );

  unlisteners.push(
    await onAgentUpdatesFinished(({ results }) => {
      // Authoritative. #1551 round 5 - a surface that showed the pass with at least
      // one result enters the persistent summary and defers its failure toasts to
      // the close; every other surface keeps the immediate toasts.
      // #1691 - the final payload MERGES MISSING COMMANDS ONLY: a result this surface
      // already observed stays the first winner.
      const merged = normalizeAgentUpdateResults(results).reduce(
        (list, result) => (hasResult(list, result.command) ? list : [...list, result]),
        state.results
      );
      const showSummary = state.inProgress && merged.length > 0;
      setAgentUpdateState({
        inProgress: false,
        prompt: null,
        running: [],
        verifying: withoutTerminal(state.verifying, merged),
        cancelRequested: withoutTerminal(state.cancelRequested, merged),
        cancelResponses: state.cancelResponses.filter((command) => !hasResult(merged, command)),
        results: merged,
        finishedSeen: true,
        summary: showSummary ? "shown" : state.summary,
      });
      if (!showSummary) showAgentUpdateFailures(merged, shownFailures);
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
      // #1691 - classify on the normalized results, never on the raw payload.
      if (!status.inProgress) {
        showAgentUpdateFailures(normalizeAgentUpdateResults(status.results), shownFailures);
      }
    })
    .catch((err) => {
      console.error("[agent-update] getStatus failed:", err);
    });

  return unlisteners;
}
