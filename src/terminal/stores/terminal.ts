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

  bindLive(selection: SessionSelection, generation: number, session: Session): boolean {
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
    setActiveWorkgroupTask(session.workgroupTask ?? null);
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

  bindLockedSession(session: Session): void {
    setSelectionId(session.id);
    setSelectionMode("live");
    setActiveSessionId(session.id);
    setActiveSessionName(session.name);
    setActiveShell(session.shell);
    setActiveShellArgs(session.effectiveShellArgs);
    setActiveWorkingDirectory(session.workingDirectory);
    setActiveWorkgroupTask(session.workgroupTask ?? null);
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

  resetForTests(): void {
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
