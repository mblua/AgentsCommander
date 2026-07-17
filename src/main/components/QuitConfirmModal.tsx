import { Component, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";

export interface QuitConfirmModalProps {
  detachedCount: number;
  onCancel: () => void;
  onQuit: () => void;
}

const QuitConfirmModal: Component<QuitConfirmModalProps> = (props) => {
  let cancelBtnRef: HTMLButtonElement | undefined;
  let quitBtnRef: HTMLButtonElement | undefined;
  let previouslyFocused: HTMLElement | null = null;

  onMount(() => {
    previouslyFocused = document.activeElement as HTMLElement | null;

    cancelBtnRef?.focus();

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        props.onCancel();
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        e.stopPropagation();
        if (document.activeElement === quitBtnRef) {
          props.onQuit();
        } else {
          props.onCancel();
        }
        return;
      }
      if (e.key === "Tab") {
        const focusables = [cancelBtnRef, quitBtnRef].filter(Boolean) as HTMLElement[];
        if (focusables.length < 2) return;
        const idx = focusables.indexOf(document.activeElement as HTMLElement);
        if (idx === -1) {
          e.preventDefault();
          (e.shiftKey ? focusables[focusables.length - 1] : focusables[0]).focus();
          return;
        }
        if (e.shiftKey) {
          if (idx <= 0) {
            e.preventDefault();
            focusables[focusables.length - 1].focus();
          }
        } else {
          if (idx === focusables.length - 1) {
            e.preventDefault();
            focusables[0].focus();
          }
        }
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => {
      document.removeEventListener("keydown", onKeyDown, true);
      try { previouslyFocused?.focus(); } catch { /* best-effort */ }
    });
  });

  return (
    <Portal>
      <div
        class="quit-confirm-backdrop"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="quit-confirm-title"
        aria-describedby="quit-confirm-body"
      >
        <div class="quit-confirm-modal">
          <h2 id="quit-confirm-title" class="quit-confirm-title">Quit AgentsCommander?</h2>
          <p id="quit-confirm-body" class="quit-confirm-body">
            You have {props.detachedCount} detached session{props.detachedCount === 1 ? "" : "s"} open.
            Quit the app and close all detached sessions?
          </p>
          <div class="quit-confirm-actions">
            <button
              ref={cancelBtnRef}
              class="quit-confirm-btn quit-confirm-btn-cancel"
              onClick={() => props.onCancel()}
              type="button"
            >
              Cancel
            </button>
            <button
              ref={quitBtnRef}
              class="quit-confirm-btn quit-confirm-btn-quit"
              onClick={() => props.onQuit()}
              type="button"
            >
              Quit
            </button>
          </div>
        </div>
      </div>
    </Portal>
  );
};

export default QuitConfirmModal;
