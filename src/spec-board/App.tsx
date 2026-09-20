import { Component, onMount, onCleanup, createEffect, createSignal, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { specBoardStore, setSpecBoardStore } from "./stores/spec-board";
import {
  QuitAPI,
  SpecBoardAPI,
  onAppQuitCancelled,
  onAppQuitOutcome,
  onAppQuitRequested,
  onSpecBoardChanged,
  onSpecBoardConflict,
  onSpecBoardFileMissing,
  type AppQuitTargetPayload,
  type QuitOutcome,
} from "../shared/ipc";
import { isTauri } from "../shared/platform";
import type { UnlistenFn } from "../shared/transport";

import SpecBoardTitlebar from "./components/SpecBoardTitlebar";
import SpecBoardToolbar from "./components/SpecBoardToolbar";
import SpecBoardEditor from "./components/SpecBoardEditor";
import MermaidPreview from "./components/MermaidPreview";
import ConflictBanner from "./components/ConflictBanner";
import AskAgentPanel from "./components/AskAgentPanel";
import SaveBeforeCloseModal from "./components/SaveBeforeCloseModal";

import "./styles/spec-board.css";

const GATE_REGISTRATION_RETRY_DELAY_MS = 1000;
const GATE_REGISTRATION_MAX_ATTEMPTS = 5;

type GateState = "registering" | "registered" | "blocked" | "failed";

const isLiveEpoch = (epoch: unknown): epoch is number =>
  typeof epoch === "number" && Number.isSafeInteger(epoch) && epoch >= 0;

const errorText = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

const SpecBoardApp: Component = () => {
  const [showCloseModal, setShowCloseModal] = createSignal(false);
  const [gateState, setGateState] = createSignal<GateState>("registering");
  const [gateError, setGateError] = createSignal<string | null>(null);
  const [pendingEpoch, setPendingEpoch] = createSignal<number | null>(null);

  let unlistenClose: (() => void) | undefined;
  let unlistens: UnlistenFn[] = [];
  let ownLabel = "spec-board";
  // High-water mark: an `Any` listener elsewhere may deliver requests that are
  // not this board's round; only strictly newer epochs may replace the modal.
  let seenEpoch = 0;
  // The active round that refused our registration (`InFlight`); its terminal
  // outcome releases the next registration retry.
  let blockedEpoch: number | null = null;
  let registerAttempts = 0;
  let registerRetryTimer: ReturnType<typeof setTimeout> | null = null;
  let progressEpoch: number | null = null;
  let controlsRef!: HTMLDivElement;
  let disposed = false;

  const boardIsDirty = (): boolean =>
    specBoardStore.dirty ||
    (!specBoardStore.path && specBoardStore.content.trim().length > 0);

  const clearRetryTimer = (): void => {
    if (registerRetryTimer !== null) {
      clearTimeout(registerRetryTimer);
      registerRetryTimer = null;
    }
  };

  const scheduleGateRetry = (): void => {
    if (disposed || registerRetryTimer !== null) return;
    if (registerAttempts >= GATE_REGISTRATION_MAX_ATTEMPTS) return;
    registerRetryTimer = setTimeout(() => {
      registerRetryTimer = null;
      void registerGate();
    }, GATE_REGISTRATION_RETRY_DELAY_MS);
  };

  const registerGate = async (): Promise<void> => {
    if (disposed) return;
    clearRetryTimer();
    registerAttempts += 1;
    setGateError(null);
    if (gateState() !== "registered") {
      setGateState("registering");
    }
    try {
      const result = await QuitAPI.registerGate();
      if (disposed) return;
      if (result && result.status === "Registered") {
        blockedEpoch = null;
        registerAttempts = 0;
        setGateState("registered");
        return;
      }
      if (result && result.status === "InFlight" && isLiveEpoch(result.epoch)) {
        // The snapshot is atomic: a late registrant would be silently ungated,
        // so editing stays disabled until the active round terminates.
        blockedEpoch = result.epoch;
        setGateState("blocked");
        scheduleGateRetry();
        return;
      }
      setGateState("failed");
      setGateError("Spec Board quit registration returned an unexpected response.");
      scheduleGateRetry();
    } catch (error) {
      if (disposed) return;
      setGateState("failed");
      setGateError(errorText(error));
      scheduleGateRetry();
    }
  };

  const retryGateRegistration = (): void => {
    if (disposed) return;
    registerAttempts = 0;
    void registerGate();
  };

  const resolveQuitConsent = async (
    epoch: number,
    consent: boolean,
  ): Promise<boolean> => {
    if (pendingEpoch() !== epoch) return false;
    if (gateState() !== "registered") return false;
    try {
      await QuitAPI.resolveGate(epoch, consent);
      return true;
    } catch (error) {
      setSpecBoardStore("renderError", errorText(error));
      return false;
    }
  };

  const beginRoundProgress = async (epoch: number): Promise<void> => {
    if (gateState() !== "registered" || pendingEpoch() !== epoch) return;
    progressEpoch = epoch;
    try {
      await QuitAPI.reportProgress(epoch, true);
    } catch (error) {
      console.error("Quit progress failed:", error);
    }
  };

  const endRoundProgress = async (epoch: number): Promise<void> => {
    if (progressEpoch !== epoch) return;
    progressEpoch = null;
    if (gateState() !== "registered") return;
    try {
      await QuitAPI.reportProgress(epoch, false);
    } catch (error) {
      console.error("Quit progress failed:", error);
    }
  };

  const clearRoundProgress = (): void => {
    if (progressEpoch === null) return;
    void endRoundProgress(progressEpoch);
  };

  const handleQuitRequested = (payload: AppQuitTargetPayload): void => {
    if (!payload || typeof payload.label !== "string" || !isLiveEpoch(payload.epoch)) {
      return;
    }
    if (payload.label !== ownLabel) return;
    if (gateState() !== "registered") return;
    if (payload.epoch <= seenEpoch) return;
    seenEpoch = payload.epoch;
    setPendingEpoch(payload.epoch);
    if (!boardIsDirty()) {
      setShowCloseModal(false);
      void resolveQuitConsent(payload.epoch, true).then((ok) => {
        if (ok) setPendingEpoch(null);
      });
      return;
    }
    setShowCloseModal(true);
  };

  const handleQuitCancelled = (payload: AppQuitTargetPayload): void => {
    if (!payload || typeof payload.label !== "string" || !isLiveEpoch(payload.epoch)) {
      return;
    }
    if (payload.label !== ownLabel) return;
    const epoch = pendingEpoch();
    if (epoch === null || epoch !== payload.epoch) return;
    setShowCloseModal(false);
    clearRoundProgress();
    setPendingEpoch(null);
  };

  const handleQuitOutcome = (payload: QuitOutcome): void => {
    if (!payload || !isLiveEpoch(payload.epoch)) return;
    if (blockedEpoch === null || blockedEpoch !== payload.epoch) return;
    if (payload.outcome === "Exiting") return; // the app is exiting; this window dies with it
    if (payload.outcome === "Aborted" || payload.outcome === "Stale") {
      blockedEpoch = null;
      registerAttempts = 0;
      void registerGate();
    }
  };

  onMount(async () => {
    // Setup listeners
    unlistens.push(await onSpecBoardChanged((payload) => {
      setSpecBoardStore({
        docId: payload.docId,
        path: payload.path,
        content: payload.content,
        diagramSource: payload.diagramSource,
        versionIndex: payload.versionIndex,
        versionCount: payload.versionCount,
        dirty: false,
        conflict: null,
      });
    }));

    unlistens.push(await onSpecBoardConflict((payload) => {
      if (specBoardStore.docId === payload.docId) {
        // Backend emits the full SpecBoardDocument on conflict
        setSpecBoardStore(payload);
      }
    }));

    unlistens.push(await onSpecBoardFileMissing((payload) => {
      if (specBoardStore.docId === payload.docId) {
        // Just mark as dirty so they can save it again
        setSpecBoardStore("dirty", true);
      }
    }));

    try {
      ownLabel = getCurrentWindow().label;
    } catch {
      ownLabel = "spec-board";
    }

    // The scoped consent listener exists BEFORE registration: the snapshot is
    // atomic and a request delivered before we can filter it would be lost.
    unlistens.push(await onAppQuitRequested(handleQuitRequested));
    unlistens.push(await onAppQuitCancelled(handleQuitCancelled));
    // UNSCOPED on purpose: while the gate is unregistered (active-round
    // `InFlight`), the targeted cancellation may never arrive, so the terminal
    // outcome addressed at main is the fallback that releases the retry.
    unlistens.push(await onAppQuitOutcome(handleQuitOutcome));

    const appWindow = getCurrentWindow();
    unlistenClose = await appWindow.onCloseRequested(async (event) => {
      event.preventDefault(); // Always intercept to handle async cleanup reliably
      if (boardIsDirty()) {
        setShowCloseModal(true);
      } else {
        await forceCloseSpecBoardWindow();
      }
    });

    if (isTauri) {
      void registerGate();
    } else {
      // No quit gate exists outside Tauri; keep the board usable.
      setGateState("registered");
    }
  });

  onCleanup(() => {
    disposed = true;
    clearRetryTimer();
    if (unlistenClose) unlistenClose();
    unlistens.forEach((u) => u());
    if (isTauri) {
      void QuitAPI.unregisterGate().catch(() => {
        /* window teardown races registration; Destroyed is the fallback */
      });
    }
  });

  const forceCloseSpecBoardWindow = async () => {
    const docId = specBoardStore.docId;
    if (unlistenClose) {
      unlistenClose();
      unlistenClose = undefined;
    }
    if (docId) {
      SpecBoardAPI.close(docId).catch(err => {
        console.warn("Best-effort backend close failed:", err);
      });
    }
    try {
      await getCurrentWindow().destroy();
    } catch (err) {
      console.warn("Window destroy failed:", err);
    }
  };

  const handleSaveAndClose = async () => {
    const epoch = pendingEpoch();
    if (specBoardStore.docId) {
      if (epoch !== null) {
        await beginRoundProgress(epoch);
      }
      try {
        if (specBoardStore.path) {
          await SpecBoardAPI.save(specBoardStore.docId, specBoardStore.content);
        } else {
          const doc = await SpecBoardAPI.pickSave(
            specBoardStore.docId,
            specBoardStore.content,
            specBoardStore.repoRoot,
          );
          if (!doc) return; // Cancelled picker stays pending
        }
        if (epoch !== null) {
          if (pendingEpoch() !== epoch) {
            // The quit was cancelled while the save ran: the stale consent is
            // ignored, its modal closes and editing resumes.
            setShowCloseModal(false);
            return;
          }
          const resolved = await resolveQuitConsent(epoch, true);
          if (!resolved) return; // Keep the modal and epoch for retry/cancel
          setShowCloseModal(false);
          setPendingEpoch(null);
          return;
        }
        // Standalone close: existing save-then-destroy behavior.
        clearRoundProgress();
        setShowCloseModal(false);
        await forceCloseSpecBoardWindow();
      } catch (err) {
        console.error("Save failed", err);
        setSpecBoardStore("renderError", String(err));
      } finally {
        if (epoch !== null) {
          void endRoundProgress(epoch);
        }
      }
    } else {
      if (epoch !== null) {
        const resolved = await resolveQuitConsent(epoch, true);
        if (!resolved) return;
        setPendingEpoch(null);
      }
      clearRoundProgress();
      setShowCloseModal(false);
      await forceCloseSpecBoardWindow();
    }
  };

  const handleDiscardClose = async () => {
    const epoch = pendingEpoch();
    if (epoch !== null) {
      // Discard during a quit round IS consent: resolve first, destroy only
      // after the backend accepted it.
      const resolved = await resolveQuitConsent(epoch, true);
      if (!resolved) return; // Board and modal stay open with the error
      setPendingEpoch(null);
      clearRoundProgress();
      setShowCloseModal(false);
      await forceCloseSpecBoardWindow();
      return;
    }
    clearRoundProgress();
    setShowCloseModal(false);
    await forceCloseSpecBoardWindow();
  };

  const handleCancelClose = () => {
    const epoch = pendingEpoch();
    setShowCloseModal(false);
    if (epoch !== null) {
      void resolveQuitConsent(epoch, false);
      setPendingEpoch(null);
      clearRoundProgress();
    }
  };

  const gateLocked = (): boolean => gateState() !== "registered";

  // Set the native attribute rather than Solid's boolean property binding:
  // the attribute is what the webview's inert behavior keys off, and jsdom
  // does not implement the property at all.
  createEffect(() => {
    controlsRef.toggleAttribute("inert", gateLocked());
  });

  return (
    <div class="spec-board-container">
      {/* Kept first and outside the inert region: the drag surface and the
       *  minimize/maximize/close buttons stay usable while registration is
       *  unresolved or rejected. */}
      <SpecBoardTitlebar />

      <Show when={gateLocked()}>
        <div class="spec-board-gate-status" role="status">
          <span>
            {gateState() === "registering"
              ? "Preparing Spec Board for quit handling..."
              : gateState() === "blocked"
                ? "Waiting for the active quit round to finish before enabling editing..."
                : `Spec Board quit registration failed${gateError() ? `: ${gateError()}` : "."}`}
          </span>
          <Show when={gateState() === "failed"}>
            <button onClick={retryGateRegistration} type="button">Retry</button>
          </Show>
        </div>
      </Show>

      <div
        class="spec-board-controls"
        ref={controlsRef}
        style={{
          display: "flex",
          "flex-direction": "column",
          flex: "1",
          "min-height": "0",
        }}
      >
        <SpecBoardToolbar />
        <ConflictBanner />

        <div class="spec-board-main">
          <SpecBoardEditor editingLocked={gateLocked()} />
          <MermaidPreview />

          <Show when={specBoardStore.showAskAgent}>
            <AskAgentPanel />
          </Show>
          <Show when={specBoardStore.renderError}>
            <div class="spec-board-error">{specBoardStore.renderError}</div>
          </Show>
        </div>

        <div class="spec-board-footer">
          <div>{specBoardStore.path || "Unsaved"} ({specBoardStore.fileKind})</div>
          <div>
            {specBoardStore.dirty ? "Dirty " : ""}
            Version {specBoardStore.versionIndex} of {specBoardStore.versionCount}
          </div>
        </div>
      </div>

      {/* Outside the inert region so an already-registered board can answer. */}
      <Show when={showCloseModal()}>
        <SaveBeforeCloseModal
          onSaveAndClose={handleSaveAndClose}
          onDiscard={handleDiscardClose}
          onCancel={handleCancelClose}
        />
      </Show>
    </div>
  );
};

export default SpecBoardApp;
