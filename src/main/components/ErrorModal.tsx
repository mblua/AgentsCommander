import { Component, createEffect, createMemo, createSignal, onCleanup, onMount, Show, untrack } from "solid-js";
import { Portal } from "solid-js/web";
import type { UnlistenFn } from "../../shared/transport";
import type { ErrorLogEntry } from "../../shared/types";
import { DebugAPI, onErrorLogEvent } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { trapTabFocus } from "../../shared/focus-trap";
import { errorModalStore } from "../stores/error-modal";

function formatEntry(e: ErrorLogEntry): string {
  return `${e.timestamp} [${e.level}] ${e.target}\n${e.message}`;
}

const ErrorModal: Component = () => {
  const [copied, setCopied] = createSignal(false);
  let messageRef: HTMLDivElement | undefined;
  let copyBtnRef: HTMLButtonElement | undefined;
  let dismissBtnRef: HTMLButtonElement | undefined;
  let copyResetTimer: ReturnType<typeof setTimeout> | null = null;
  let previouslyFocused: HTMLElement | null = null;
  let wasOpen = false;
  let muteRef: HTMLInputElement | undefined;
  const unlisteners: UnlistenFn[] = [];
  let disposed = false;

  // #3037 - popup mute. The backend owns the muted set; this instance only
  // mirrors the last confirmed snapshot. `synced` is false until a snapshot
  // succeeds and after a failed reconciliation; `syncFailed` drives the status.
  const [mutedIds, setMutedIds] = createSignal<readonly string[]>([]);
  const [synced, setSynced] = createSignal(false);
  const [syncFailed, setSyncFailed] = createSignal(false);
  const [mutePending, setMutePending] = createSignal(false);
  let retryQueued = false;

  const currentEntry = createMemo(() => errorModalStore.current);
  const currentTypeId = () => currentEntry()?.typeId || null;
  const muteChecked = () => {
    const id = currentTypeId();
    return id !== null && mutedIds().includes(id);
  };
  const muteDisabled = () => !synced() || mutePending() || currentTypeId() === null;
  const showSyncStatus = () => syncFailed() || (currentEntry() !== null && currentTypeId() === null);

  // One chain serializes snapshot/drain/set of this instance; each step owns
  // its errors, so a rejection never breaks the chain.
  let chain: Promise<void> = Promise.resolve();
  const schedule = (step: () => Promise<void>) => {
    chain = chain.then(() => (disposed ? undefined : step())).catch((err) => {
      console.error("[error-modal] queued step failed:", err);
    });
  };

  const refreshSnapshot = async () => {
    retryQueued = false;
    try {
      const ids = await DebugAPI.getErrorPopupMutes();
      if (disposed) return;
      setMutedIds(ids);
      setSynced(true);
      setSyncFailed(false);
    } catch (err) {
      if (disposed) return;
      console.error("[error-modal] getErrorPopupMutes failed:", err);
      setSynced(false);
      setSyncFailed(true);
    }
  };

  const retrySnapshotIfFailed = () => {
    if (!syncFailed() || retryQueued || mutePending()) return;
    retryQueued = true;
    schedule(refreshSnapshot);
  };

  const drainAndEnqueue = async () => {
    try {
      const entries = await DebugAPI.drainErrorLogs();
      if (disposed) return;
      errorModalStore.enqueue(entries);
    } catch (err) {
      if (disposed) return;
      console.error("[error-modal] drainErrorLogs failed:", err);
    }
  };

  const onErrorPing = () => {
    if (disposed) return;
    retrySnapshotIfFailed();
    schedule(drainAndEnqueue);
  };

  onMount(async () => {
    if (!isTauri) return; // Web remote client: the error modal is desktop-only.
    const unlisten = await onErrorLogEvent(onErrorPing);
    if (disposed) {
      unlisten();
      return;
    }
    unlisteners.push(unlisten);
    schedule(refreshSnapshot);
    schedule(drainAndEnqueue);
  });

  const onMuteChange = (e: Event & { currentTarget: HTMLInputElement }) => {
    // Captured at the event: never re-read `current` after an await.
    const typeId = currentTypeId();
    const muted = e.currentTarget.checked;
    // No optimistic success: the box shows only the confirmed set.
    e.currentTarget.checked = muteChecked();
    if (typeId === null || muteDisabled()) return;
    setMutePending(true);
    schedule(async () => {
      try {
        const ids = await DebugAPI.setErrorPopupMuted(typeId, muted);
        if (disposed) return;
        setMutedIds(ids);
        setSynced(true);
        setSyncFailed(false);
      } catch (err) {
        if (disposed) return;
        console.error("[error-modal] setErrorPopupMuted failed:", err);
        await refreshSnapshot();
      } finally {
        if (!disposed) setMutePending(false);
      }
    });
  };

  createEffect(() => {
    currentEntry();
    untrack(retrySnapshotIfFailed);
  });

  onMount(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!errorModalStore.open) return; // closed: let keys through untouched
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        errorModalStore.dismissCurrent();
        return;
      }
      if (e.key === "Tab") {
        e.stopImmediatePropagation();
        const mute = muteRef && !muteRef.disabled ? muteRef : undefined;
        trapTabFocus(e, [messageRef, mute, copyBtnRef, dismissBtnRef].filter(Boolean) as HTMLElement[]);
        return;
      }
      e.stopImmediatePropagation();
    };
    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => document.removeEventListener("keydown", onKeyDown, true));
  });

  onCleanup(() => {
    disposed = true; // late responses and a late listener registration are ignored
    unlisteners.forEach((u) => u());
    if (copyResetTimer) clearTimeout(copyResetTimer);
  });

  createEffect(() => {
    const open = errorModalStore.open;
    if (open === wasOpen) return;
    wasOpen = open;
    if (open) {
      previouslyFocused = document.activeElement as HTMLElement | null;
      if (isTauri && !document.hasFocus()) {
        void (async () => {
          try {
            const { getCurrentWindow, UserAttentionType } =
              await import("@tauri-apps/api/window");
            await getCurrentWindow().requestUserAttention(UserAttentionType.Critical);
          } catch (err) {
            console.error("[error-modal] requestUserAttention failed:", err);
          }
        })();
      }
    } else {
      try { previouslyFocused?.focus(); } catch { /* best-effort */ }
      previouslyFocused = null;
    }
  });

  createEffect(() => {
    const cur = currentEntry();
    setCopied(false);
    if (cur) queueMicrotask(() => dismissBtnRef?.focus());
  });

  const onCopy = async () => {
    const cur = currentEntry();
    if (!cur) return;
    try {
      await navigator.clipboard.writeText(formatEntry(cur));
      setCopied(true);
      if (copyResetTimer) clearTimeout(copyResetTimer);
      copyResetTimer = setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      console.error("[error-modal] clipboard write failed:", err);
    }
  };

  return (
    <Show when={errorModalStore.open}>
      <Portal>
        <div
          class="error-modal-backdrop"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="error-modal-title"
          aria-describedby="error-modal-message"
        >
          <div class="error-modal">
            <div class="error-modal-header">
              <h2 id="error-modal-title" class="error-modal-title">Application Error</h2>
              <Show when={errorModalStore.total > 1}>
                <span class="error-modal-counter">
                  {errorModalStore.index + 1} of {errorModalStore.total}
                </span>
              </Show>
            </div>
            <div class="error-modal-meta">
              <span>{currentEntry()?.timestamp}</span>
              <span>{currentEntry()?.target}</span>
            </div>
            <div
              id="error-modal-message"
              class="error-modal-message"
              ref={messageRef}
              tabindex="0"
              role="region"
              aria-label="Error detail"
            >
              {currentEntry()?.message}
            </div>
            <div class="error-mute-field">
              <label class="settings-checkbox-field">
                <input
                  ref={muteRef}
                  type="checkbox"
                  class="settings-checkbox"
                  aria-describedby="error-mute-help"
                  checked={muteChecked()}
                  disabled={muteDisabled()}
                  onChange={onMuteChange}
                />
                <span>Hide future popups for this error type until the application closes</span>
              </label>
              <p id="error-mute-help" class="error-mute-help">Errors will still be logged.</p>
              <p class="error-mute-status" role="status" aria-live="polite">
                {showSyncStatus() ? "Could not confirm popup preference." : ""}
              </p>
            </div>
            <div class="error-modal-actions">
              <button
                ref={copyBtnRef}
                class="error-modal-btn error-modal-btn-copy"
                type="button"
                onClick={onCopy}
              >
                {copied() ? "Copied!" : "Copy"}
              </button>
              <button
                ref={dismissBtnRef}
                class="error-modal-btn error-modal-btn-dismiss"
                type="button"
                onClick={() => errorModalStore.dismissCurrent()}
              >
                Dismiss
              </button>
            </div>
          </div>
        </div>
      </Portal>
    </Show>
  );
};

export default ErrorModal;
