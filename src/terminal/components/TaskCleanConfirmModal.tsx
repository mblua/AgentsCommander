import { Component, onMount, onCleanup } from "solid-js";
import { trapTabFocus } from "../../shared/focus-trap";

export interface TaskCleanConfirmModalProps {
  onCancel: () => void;
  onConfirm: () => void;
}

const TaskCleanConfirmModal: Component<TaskCleanConfirmModalProps> = (props) => {
  let cancelBtnRef: HTMLButtonElement | undefined;
  let confirmBtnRef: HTMLButtonElement | undefined;
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
        if (document.activeElement === confirmBtnRef) {
          props.onConfirm();
        } else {
          props.onCancel();
        }
        return;
      }
      if (e.key === "Tab") {
        trapTabFocus(e, [cancelBtnRef, confirmBtnRef].filter(Boolean) as HTMLElement[]);
      }
    };
    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => {
      document.removeEventListener("keydown", onKeyDown, true);
      try { previouslyFocused?.focus(); } catch { /* best-effort */ }
    });
  });

  return (
    <div
      data-ac-testid="taskCleanConfirm.root" data-ac-role="dialog" class="quit-confirm-backdrop"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="task-clean-title"
      aria-describedby="task-clean-body"
    >
      <div class="quit-confirm-modal">
        <h2 data-ac-testid="taskCleanConfirm.title" data-ac-role="surface" id="task-clean-title" class="quit-confirm-title">Clean TASK?</h2>
        <p data-ac-testid="taskCleanConfirm.body" data-ac-role="surface" id="task-clean-body" class="quit-confirm-body">
          Back up the task and its history with the same timestamp, then start a new topic?
        </p>
        <div class="quit-confirm-actions">
          <button
            data-ac-testid="taskCleanConfirm.cancel" data-ac-role="button" ref={cancelBtnRef}
            class="quit-confirm-btn quit-confirm-btn-cancel"
            onClick={() => props.onCancel()}
            type="button"
          >
            Cancel
          </button>
          <button
            data-ac-testid="taskCleanConfirm.confirm" data-ac-role="button" ref={confirmBtnRef}
            class="quit-confirm-btn quit-confirm-btn-quit"
            onClick={() => props.onConfirm()}
            type="button"
          >
            Clean
          </button>
        </div>
      </div>
    </div>
  );
};

export default TaskCleanConfirmModal;
