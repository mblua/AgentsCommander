import { createSignal } from "solid-js";
import type { Session, SessionSelection, SessionSelectionMode } from "../../shared/types";
import type { TransportConnectionState } from "../../shared/transport";

export type TerminalBindingState = "pending" | "bound" | "unavailable";

const [selectionId, setSelectionId] = createSignal<string | null>(null);
const [selectionMode, setSelectionMode] = createSignal<SessionSelectionMode>("none");
const [selectionEpoch, setSelectionEpoch] = createSignal<string | null>(null);
const [appliedRevision, setAppliedRevision] = createSignal(-1);
const [retiredEpochs, setRetiredEpochs] = createSignal<ReadonlySet<string>>(new Set());
const [connectionGeneration, setConnectionGeneration] = createSignal(-1);
const [transportConnected, setTransportConnected] = createSignal(false);
const [awaitingHydrationGeneration, setAwaitingHydrationGeneration] = createSignal<number | null>(null);
const [bindingState, setBindingState] = createSignal<TerminalBindingState>("unavailable");
const [activeSessionId, setActiveSessionId] = createSignal<string | null>(null);
const [activeSessionName, setActiveSessionName] = createSignal("");
const [activeShell, setActiveShell] = createSignal("");
const [activeShellArgs, setActiveShellArgs] = createSignal<string[] | null>(null);
const [activeWorkingDirectory, setActiveWorkingDirectory] = createSignal("");
const [activeWorkgroupTask, setActiveWorkgroupTask] = createSignal<string | null>(null);
const [activeIsRootAgent, setActiveIsRootAgent] = createSignal(false);

// #1455 - `activeWorkgroupTask` is a pure cache with no periodic refresh, so its two
// asynchronous writers (a local TASK mutation and a `SessionAPI.list()` snapshot)
// used to resolve last-write-wins, and a snapshot taken before a save could revert
// the header after it. These two values give the writers an order. Deliberately NOT
// signals: they are race tokens that are never rendered, and a reactive read from a
// memo would be a subscription nobody wants.
let taskWriteSeq = 0;
let lastLocalTaskWrite: { workgroupRoot: string; seq: number } | null = null;

// #1455 - TASK.md is per-WORKGROUP, not per-session: `find_workgroup_task_path_for_cwd`
// (src-tauri/src/session/session.rs:242-256) walks up from a session's cwd to the first
// `wg-*` ancestor, and `SessionInfo.workgroup_task` re-reads that path on every list
// (`session.rs:396`), so every session under one workgroup root shows the same file.
// Ownership of a task write is therefore a workgroup question, never a session one.
// This is the same normalise-and-prefix comparison the manual-event handler already
// uses at src/terminal/App.tsx:326-347; the two must stay in agreement.
function normalizeTaskPath(path: string): string {
  let normalized = path;
  if (normalized.startsWith("\\\\?\\")) normalized = normalized.slice(4);
  else if (normalized.startsWith("//?/")) normalized = normalized.slice(4);
  return normalized.replace(/\\/g, "/").toLowerCase();
}

function cwdUnderWorkgroupRoot(cwd: string, workgroupRoot: string): boolean {
  if (!cwd || !workgroupRoot) return false;
  const normalizedCwd = normalizeTaskPath(cwd);
  const normalizedRoot = normalizeTaskPath(workgroupRoot);
  return (
    normalizedCwd === normalizedRoot ||
    normalizedCwd.startsWith(`${normalizedRoot}/`)
  );
}

// #1455 - true when an accepted local task write landed after the caller took its
// session snapshot AND that session displays the same TASK.md, i.e. the snapshot's
// task field is already stale.
function localTaskWriteWins(sessionCwd: string, expectedTaskSeq: number): boolean {
  return (
    lastLocalTaskWrite !== null &&
    lastLocalTaskWrite.seq > expectedTaskSeq &&
    cwdUnderWorkgroupRoot(sessionCwd, lastLocalTaskWrite.workgroupRoot)
  );
}

function clearLiveMetadata(): void {
  setActiveSessionId(null);
  setActiveSessionName("");
  setActiveShell("");
  setActiveShellArgs(null);
  setActiveWorkingDirectory("");
  setActiveWorkgroupTask(null);
  setActiveIsRootAgent(false);
}

function matchesCurrentSelection(
  selection: SessionSelection,
  generation: number,
): boolean {
  return (
    connectionGeneration() === generation &&
    transportConnected() &&
    selectionEpoch() === selection.epoch &&
    appliedRevision() === selection.revision &&
    selectionId() === selection.id &&
    selectionMode() === selection.mode
  );
}

export const terminalStore = {
  get selectionId() {
    return selectionId();
  },
  get selectionMode() {
    return selectionMode();
  },
  get selectionEpoch() {
    return selectionEpoch();
  },
  get appliedRevision() {
    return appliedRevision();
  },
  get retiredEpochs() {
    return retiredEpochs();
  },
  get connectionGeneration() {
    return connectionGeneration();
  },
  get transportConnected() {
    return transportConnected();
  },
  get awaitingHydrationGeneration() {
    return awaitingHydrationGeneration();
  },
  get bindingState() {
    return bindingState();
  },
  get activeSessionId() {
    return activeSessionId();
  },
  get activeSessionName() {
    return activeSessionName();
  },
  get activeShell() {
    return activeShell();
  },
  get activeShellArgs() {
    return activeShellArgs();
  },
  get activeWorkingDirectory() {
    return activeWorkingDirectory();
  },
  get activeWorkgroupTask() {
    return activeWorkgroupTask();
  },
  get activeIsRootAgent() {
    return activeIsRootAgent();
  },

  observeConnection(state: TransportConnectionState): boolean {
    if (state.generation < connectionGeneration()) return false;
    const generationChanged = state.generation !== connectionGeneration();
    const changed =
      generationChanged ||
      (state.state === "connected") !== transportConnected();
    setConnectionGeneration(state.generation);
    setTransportConnected(state.state === "connected");
    if (state.state === "disconnected" || generationChanged) {
      setAwaitingHydrationGeneration(null);
      clearLiveMetadata();
      setBindingState("unavailable");
    }
    return changed;
  },

  beginHydration(generation: number): boolean {
    if (generation !== connectionGeneration() || !transportConnected()) return false;
    setAwaitingHydrationGeneration(generation);
    return true;
  },

  cancelHydration(generation?: number): void {
    if (
      generation === undefined ||
      awaitingHydrationGeneration() === generation
    ) {
      setAwaitingHydrationGeneration(null);
    }
  },

  reserveSelection(
    selection: SessionSelection,
    generation: number,
    allowEqualReconnect: boolean,
  ): boolean {
    if (generation !== connectionGeneration() || !transportConnected()) return false;

    const currentEpoch = selectionEpoch();
    if (currentEpoch === selection.epoch) {
      if (selection.revision < appliedRevision()) return false;
      if (selection.revision === appliedRevision()) {
        if (
          !allowEqualReconnect ||
          awaitingHydrationGeneration() !== generation
        ) {
          return false;
        }
      }
    } else {
      if (retiredEpochs().has(selection.epoch)) return false;
      if (currentEpoch) {
        setRetiredEpochs((previous) => new Set([...previous, currentEpoch]));
      }
    }

    setSelectionEpoch(selection.epoch);
    setAppliedRevision(selection.revision);
    setSelectionId(selection.id);
    setSelectionMode(selection.mode);
    setAwaitingHydrationGeneration(null);
    clearLiveMetadata();
    setBindingState(selection.mode === "live" ? "pending" : "unavailable");
    return true;
  },

  matchesSelection(selection: SessionSelection, generation: number): boolean {
    return matchesCurrentSelection(selection, generation);
  },

  bindLive(
    selection: SessionSelection,
    generation: number,
    session: Session,
    expectedTaskSeq: number,
  ): boolean {
    if (
      selection.mode !== "live" ||
      session.id !== selection.id ||
      typeof session.status !== "string" ||
      !matchesCurrentSelection(selection, generation)
    ) {
      return false;
    }
    setActiveSessionId(session.id);
    setActiveSessionName(session.name);
    setActiveShell(session.shell);
    setActiveShellArgs(session.effectiveShellArgs);
    setActiveWorkingDirectory(session.workingDirectory);
    // #1455 - every other field binds unconditionally; only the task field can lose
    // to a newer local write against the same workgroup's TASK.md.
    if (!localTaskWriteWins(session.workingDirectory, expectedTaskSeq)) {
      setActiveWorkgroupTask(session.workgroupTask ?? null);
    }
    setActiveIsRootAgent(session.isRootAgent);
    setBindingState("bound");
    return true;
  },

  markUnavailable(selection: SessionSelection, generation: number): void {
    if (!matchesCurrentSelection(selection, generation)) return;
    clearLiveMetadata();
    setBindingState("unavailable");
  },

  safetySuspendDestroyed(id: string): boolean {
    if (activeSessionId() !== id) return false;
    clearLiveMetadata();
    if (selectionMode() === "live" && selectionId() === id) {
      setBindingState("pending");
    } else {
      setBindingState("unavailable");
    }
    return true;
  },

  suspendLiveBinding(): void {
    clearLiveMetadata();
    setBindingState("unavailable");
  },

  bindLockedSession(session: Session, expectedTaskSeq: number): void {
    setSelectionId(session.id);
    setSelectionMode("live");
    setActiveSessionId(session.id);
    setActiveSessionName(session.name);
    setActiveShell(session.shell);
    setActiveShellArgs(session.effectiveShellArgs);
    setActiveWorkingDirectory(session.workingDirectory);
    // #1455 - see bindLive.
    if (!localTaskWriteWins(session.workingDirectory, expectedTaskSeq)) {
      setActiveWorkgroupTask(session.workgroupTask ?? null);
    }
    setActiveIsRootAgent(session.isRootAgent);
    setBindingState("bound");
  },

  clearLockedSession(): void {
    setSelectionId(null);
    setSelectionMode("none");
    clearLiveMetadata();
    setBindingState("unavailable");
  },

  renameBoundSession(id: string, name: string): void {
    if (activeSessionId() === id) setActiveSessionName(name);
  },

  setActiveWorkgroupTask(task: string | null): void {
    setActiveWorkgroupTask(task);
  },

  // #1455 - capture this immediately BEFORE an awaited `SessionAPI.list()` and hand
  // it back to `bindLive` / `bindLockedSession`. Non-reactive on purpose; see the
  // declaration.
  get taskWriteSeq() {
    return taskWriteSeq;
  },

  // #1455 - the local-write side of the task ordering. `workgroupRoot` is the root
  // whose TASK.md the mutation just rewrote, straight off `TaskUpdateResult`. The
  // write is accepted unless the header is positively known to be showing a
  // DIFFERENT workgroup's file. While the store is unbound (`clearLiveMetadata`
  // blanks the cwd on every selection reserve and every transport generation change)
  // the workgroup cannot be resolved, so the write is accepted and the bind that
  // blanked it decides the final value through `localTaskWriteWins`.
  applyLocalTask(workgroupRoot: string, task: string | null): void {
    const cwd = activeWorkingDirectory();
    if (cwd && !cwdUnderWorkgroupRoot(cwd, workgroupRoot)) return;
    taskWriteSeq += 1;
    lastLocalTaskWrite = { workgroupRoot, seq: taskWriteSeq };
    setActiveWorkgroupTask(task);
  },

  resetForTests(): void {
    taskWriteSeq = 0;
    lastLocalTaskWrite = null;
    setSelectionId(null);
    setSelectionMode("none");
    setSelectionEpoch(null);
    setAppliedRevision(-1);
    setRetiredEpochs(new Set<string>());
    setConnectionGeneration(-1);
    setTransportConnected(false);
    setAwaitingHydrationGeneration(null);
    clearLiveMetadata();
    setBindingState("unavailable");
  },

  setActiveSessionForTests(id: string | null): void {
    if (import.meta.env.MODE !== "test") {
      throw new Error("setActiveSessionForTests is test-only");
    }
    setSelectionId(id);
    setSelectionMode(id ? "live" : "none");
    setActiveSessionId(id);
    setBindingState(id ? "bound" : "unavailable");
    if (!id) clearLiveMetadata();
  },
};
