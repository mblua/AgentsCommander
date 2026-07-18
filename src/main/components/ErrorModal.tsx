import { Component, createEffect, createMemo, createSignal, onCleanup, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import type { UnlistenFn } from "../../shared/transport";
import type { ErrorLogEntry } from "../../shared/types";
import { DebugAPI, onErrorLogEvent } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
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
  const unlisteners: UnlistenFn[] = [];

  const currentEntry = createMemo(() => errorModalStore.current);

  const drainAndEnqueue = async () => {
    try {
      errorModalStore.enqueue(await DebugAPI.drainErrorLogs());
    } catch (err) {
      console.error("[error-modal] drainErrorLogs failed:", err);
    }
  };

  onMount(async () => {
    if (!isTauri) return; // Web remote client: the error modal is desktop-only.
    unlisteners.push(await onErrorLogEvent(drainAndEnqueue));
    await drainAndEnqueue();
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
        const focusables = [messageRef, copyBtnRef, dismissBtnRef].filter(
          Boolean
        ) as HTMLElement[];
        if (focusables.length < 2) return;
        const idx = focusables.indexOf(document.activeElement as HTMLElement);
        if (idx === -1) {
          e.preventDefault();
          (e.shiftKey ? focusables[focusables.length - 1] : focusables[0]).focus();
          return;
        }
        if (e.shiftKey) {
          if (idx <= 0) { e.preventDefault(); focusables[focusables.length - 1].focus(); }
        } else {
          if (idx === focusables.length - 1) { e.preventDefault(); focusables[0].focus(); }
        }
        return;
      }
      e.stopImmediatePropagation();
    };
    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => document.removeEventListener("keydown", onKeyDown, true));
  });

  onCleanup(() => {
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
