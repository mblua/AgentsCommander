import { Component, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { trapTabFocus } from "../../shared/focus-trap";

interface QuitConfirmCommonProps {
  /**
   * Read at CLEANUP time, never captured as a boolean: the parent makes it
   * false before a terminal round unmounts the modal, and leaves it true while
   * a Force dialog is merely declined. Defaults to restoring focus.
   */
  shouldRestoreFocus?: () => boolean;
}

/** The existing pre-quit confirmation shown when detached sessions are open. */
export interface DetachedQuitConfirmModalProps extends QuitConfirmCommonProps {
  mode?: "detached";
  detachedCount: number;
  onCancel: () => void;
  onQuit: () => void;
}

/** The separate possible-loss warning shown behind a bound live epoch. */
export interface ForceQuitConfirmModalProps extends QuitConfirmCommonProps {
  mode: "force";
  onKeepWaiting: () => void;
  onForceQuit: () => void;
}

export type QuitConfirmModalProps =
  | DetachedQuitConfirmModalProps
  | ForceQuitConfirmModalProps;

const QuitConfirmModal: Component<QuitConfirmModalProps> = (props) => {
  let cancelBtnRef: HTMLButtonElement | undefined;
  let quitBtnRef: HTMLButtonElement | undefined;
  let keepWaitingBtnRef: HTMLButtonElement | undefined;
  let forceQuitBtnRef: HTMLButtonElement | undefined;
  let previouslyFocused: HTMLElement | null = null;

  onMount(() => {
    previouslyFocused = (document.activeElement as HTMLElement | null) ?? null;

    if (props.mode === "force") {
      keepWaitingBtnRef?.focus();
    } else {
      cancelBtnRef?.focus();
    }

    const onKeyDown = (e: KeyboardEvent) => {
      // Keys are routed by the ACTIVE mode only. A ref left behind by the
      // other variant can never steer this handler.
      if (props.mode === "force") {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          props.onKeepWaiting();
          return;
        }
        if (e.key === "Enter") {
          e.preventDefault();
          e.stopPropagation();
          if (document.activeElement === forceQuitBtnRef) {
            props.onForceQuit();
          } else {
            props.onKeepWaiting();
          }
          return;
        }
        if (e.key === "Tab") {
          trapTabFocus(
            e,
            [keepWaitingBtnRef, forceQuitBtnRef].filter(Boolean) as HTMLElement[],
          );
        }
        return;
      }

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
        trapTabFocus(
          e,
          [cancelBtnRef, quitBtnRef].filter(Boolean) as HTMLElement[],
        );
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => {
      document.removeEventListener("keydown", onKeyDown, true);
      const shouldRestore = props.shouldRestoreFocus?.() ?? true;
      if (!shouldRestore) {
        return;
      }
      try {
        previouslyFocused?.focus();
      } catch {
        /* best-effort */
      }
    });
  });

  return (
    <Portal>
      {props.mode === "force" ? (
        <div
          class="quit-confirm-backdrop"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="quit-force-title"
          aria-describedby="quit-force-body"
        >
          <div class="quit-confirm-modal">
            <h2 id="quit-force-title" class="quit-confirm-title">Force quit?</h2>
            <p id="quit-force-body" class="quit-confirm-body">
              Unsaved Spec Board changes will be lost. If an atomic save is
              being written right now, interrupting it may lose that save too.
            </p>
            <div class="quit-confirm-actions">
              <button
                ref={keepWaitingBtnRef}
                class="quit-confirm-btn quit-confirm-btn-cancel"
                data-ac-testid="quit.keepWaiting"
                onClick={() => props.onKeepWaiting()}
                type="button"
              >
                Keep waiting
              </button>
              <button
                ref={forceQuitBtnRef}
                class="quit-confirm-btn quit-confirm-btn-quit"
                data-ac-testid="quit.forceConfirm"
                onClick={() => props.onForceQuit()}
                type="button"
              >
                Force quit
              </button>
            </div>
          </div>
        </div>
      ) : (
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
                data-ac-testid="quit.cancel"
                onClick={() => props.onCancel()}
                type="button"
              >
                Cancel
              </button>
              <button
                ref={quitBtnRef}
                class="quit-confirm-btn quit-confirm-btn-quit"
                data-ac-testid="quit.confirm"
                onClick={() => props.onQuit()}
                type="button"
              >
                Quit
              </button>
            </div>
          </div>
        </div>
      )}
    </Portal>
  );
};

export default QuitConfirmModal;
