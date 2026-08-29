import { Component, onMount, onCleanup } from "solid-js";

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
        const focusables = [cancelBtnRef, confirmBtnRef].filter(Boolean) as HTMLElement[];
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
        } else if (idx === focusables.length - 1) {
          e.preventDefault();
          focusables[0].focus();
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
    <div
      class="quit-confirm-backdrop"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="task-clean-title"
      aria-describedby="task-clean-body"
    >
      <div class="quit-confirm-modal">
        <h2 id="task-clean-title" class="quit-confirm-title">Clean TASK?</h2>
        <p id="task-clean-body" class="quit-confirm-body">
          This <strong>resets</strong> the room TASK.md — all frontmatter fields
          and body content are replaced with <code>title: 'Clean'</code> and body
          <code>Ready to start a new topic</code>. If a TASK.md exists, a timestamped
          backup is saved alongside it. Continue?
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
            ref={confirmBtnRef}
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
