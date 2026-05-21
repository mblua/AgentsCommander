import { Component, createEffect, createMemo, createSignal, onCleanup, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import type { UnlistenFn } from "../../shared/transport";
import type { ErrorLogEntry } from "../../shared/types";
import { DebugAPI, onErrorLogEvent } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { errorModalStore } from "../stores/error-modal";

/** Copy-to-clipboard text for one entry — header line + full message. */
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
  const unlisteners: UnlistenFn[] = [];

  // H1: a memo notifies dependents only when the entry's *identity* changes.
  // Reading errorModalStore.current directly in an effect re-fires whenever
  // entries() changes — including when a background error is enqueued behind an
  // already-open modal — which would steal focus and reset "Copied!". `enqueue`
  // only appends, so an already-shown entry keeps its object reference and the
  // memo stays quiet; it fires only on real transitions (open, advance, close).
  const currentEntry = createMemo(() => errorModalStore.current);

  // --- Backend feed: listen for pings, drain on each + once on mount. -------
  const drainAndEnqueue = async () => {
    try {
      errorModalStore.enqueue(await DebugAPI.drainErrorLogs());
    } catch (err) {
      console.error("[error-modal] drainErrorLogs failed:", err);
    }
  };

  onMount(async () => {
    if (!isTauri) return; // Web remote client: the error modal is desktop-only.
    // Subscribe BEFORE the initial drain so a ping fired mid-mount is not lost
    // (same ordering rule as RtkBanner). A ping racing the drain just triggers
    // a second drain — harmless: read-and-clear returns [] when already empty.
    unlisteners.push(await onErrorLogEvent(drainAndEnqueue));
    await drainAndEnqueue();
  });

  // --- Keyboard: ESC dismisses, Tab traps; modal owns the keyboard. --------
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
        // H2: the scrollable message box is in the trap, in DOM order, so a
        // keyboard-only user can Tab to it and scroll a long multi-line error.
        const focusables = [messageRef, copyBtnRef, dismissBtnRef].filter(
          Boolean
        ) as HTMLElement[];
        if (focusables.length < 2) return;
        const idx = focusables.indexOf(document.activeElement as HTMLElement);
        if (e.shiftKey) {
          if (idx <= 0) { e.preventDefault(); focusables[focusables.length - 1].focus(); }
        } else {
          if (idx === focusables.length - 1) { e.preventDefault(); focusables[0].focus(); }
        }
        return;
      }
      // Any other key while open (incl. Enter): swallow propagation so xterm.js
      // / global shortcuts do not react behind the modal. NOT preventDefault —
      // native Enter/Space still activates the focused button, and arrow keys
      // still scroll the focused message box (default action).
      e.stopImmediatePropagation();
    };
    // Capture phase + stopImmediatePropagation. ErrorModal mounts before
    // QuitConfirmModal ever does, so its capture listener is registered first
    // and wins — while the error modal is open it fully owns the keyboard.
    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => document.removeEventListener("keydown", onKeyDown, true));
  });

  onCleanup(() => {
    unlisteners.forEach((u) => u());
    if (copyResetTimer) clearTimeout(copyResetTimer);
  });

  // --- On closed->open while unfocused: flash the taskbar. Track focus owner.
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

  // --- On open / advance to next entry: focus Dismiss, reset Copy state. ----
  // Tracks currentEntry() (the H1 memo) so it does NOT re-fire when a
  // background error is enqueued behind an already-open modal.
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
              <span class="error-modal-meta-time">{currentEntry()?.timestamp}</span>
              <span class="error-modal-meta-target">{currentEntry()?.target}</span>
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
