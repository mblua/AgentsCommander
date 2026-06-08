import { Component, createSignal, onCleanup, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { WindowAPI } from "../ipc";
import { getConfirmableExternalUrl } from "../external-links";

const ExternalLinkConfirm: Component = () => {
  const [pendingUrl, setPendingUrl] = createSignal<string | null>(null);
  const [copied, setCopied] = createSignal(false);
  let cancelBtnRef: HTMLButtonElement | undefined;
  let copyBtnRef: HTMLButtonElement | undefined;
  let openBtnRef: HTMLButtonElement | undefined;
  let copyResetTimer: ReturnType<typeof setTimeout> | null = null;
  let previouslyFocused: HTMLElement | null = null;

  const close = () => {
    setPendingUrl(null);
    setCopied(false);
    if (copyResetTimer) {
      clearTimeout(copyResetTimer);
      copyResetTimer = null;
    }
    try { previouslyFocused?.focus(); } catch { /* best-effort */ }
    previouslyFocused = null;
  };

  const openPendingUrl = async () => {
    const url = pendingUrl();
    if (!url) return;
    try {
      await WindowAPI.openExternal(url);
      close();
    } catch (err) {
      console.error("[external-link-confirm] openExternal failed:", err);
    }
  };

  const copyPendingUrl = async () => {
    const url = pendingUrl();
    if (!url) return;
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      if (copyResetTimer) clearTimeout(copyResetTimer);
      copyResetTimer = setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      console.error("[external-link-confirm] clipboard write failed:", err);
    }
  };

  onMount(() => {
    const onClick = (e: MouseEvent) => {
      if (e.defaultPrevented || e.button !== 0) {
        return;
      }
      const target = e.target as HTMLElement | null;
      const anchor = target?.closest("a") as HTMLAnchorElement | null;
      const href = anchor?.getAttribute("href");
      if (!href) return;

      const url = getConfirmableExternalUrl(href);
      if (!url) return;

      e.preventDefault();
      e.stopImmediatePropagation();
      previouslyFocused = document.activeElement as HTMLElement | null;
      setCopied(false);
      setPendingUrl(url);
      queueMicrotask(() => cancelBtnRef?.focus());
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (!pendingUrl()) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        close();
        return;
      }
      if (e.key === "Tab") {
        e.stopImmediatePropagation();
        const focusables = [copyBtnRef, cancelBtnRef, openBtnRef].filter(Boolean) as HTMLElement[];
        if (focusables.length < 2) return;
        const idx = focusables.indexOf(document.activeElement as HTMLElement);
        if (idx === -1) {
          e.preventDefault();
          (e.shiftKey ? focusables[focusables.length - 1] : focusables[0]).focus();
          return;
        }
        if (e.shiftKey && idx <= 0) {
          e.preventDefault();
          focusables[focusables.length - 1].focus();
        } else if (!e.shiftKey && idx === focusables.length - 1) {
          e.preventDefault();
          focusables[0].focus();
        }
        return;
      }
    };

    document.addEventListener("click", onClick, true);
    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => {
      document.removeEventListener("click", onClick, true);
      document.removeEventListener("keydown", onKeyDown, true);
    });
  });

  onCleanup(() => {
    if (copyResetTimer) clearTimeout(copyResetTimer);
  });

  return (
    <Show when={pendingUrl()}>
      {(url) => (
        <Portal>
          <div
            class="external-link-backdrop"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="external-link-title"
            aria-describedby="external-link-body"
          >
            <div class="external-link-modal">
              <div class="external-link-header">
                <h2 id="external-link-title" class="external-link-title">Open external link?</h2>
                <button
                  ref={copyBtnRef}
                  class="external-link-copy-btn"
                  type="button"
                  title={copied() ? "Copied" : "Copy URL"}
                  aria-label={copied() ? "Copied URL" : "Copy URL"}
                  onClick={copyPendingUrl}
                >
                  {copied() ? "✓" : "⧉"}
                </button>
              </div>
              <p id="external-link-body" class="external-link-body">
                This link opens outside Agents Commander.
              </p>
              <div class="external-link-url" title={url()}>
                {url()}
              </div>
              <div class="external-link-actions">
                <button
                  ref={cancelBtnRef}
                  class="external-link-btn external-link-btn-cancel"
                  type="button"
                  onClick={close}
                >
                  Cancel
                </button>
                <button
                  ref={openBtnRef}
                  class="external-link-btn external-link-btn-open"
                  type="button"
                  onClick={openPendingUrl}
                >
                  Open anyway
                </button>
              </div>
            </div>
          </div>
        </Portal>
      )}
    </Show>
  );
};

export default ExternalLinkConfirm;
