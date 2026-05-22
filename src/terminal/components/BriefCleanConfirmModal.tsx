import { Component, onMount, onCleanup } from "solid-js";

export interface BriefCleanConfirmModalProps {
  onCancel: () => void;
  onConfirm: () => void;
}

const BriefCleanConfirmModal: Component<BriefCleanConfirmModalProps> = (props) => {
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
        // #272: activeElement can be outside the trap entirely — e.g. blurred
        // to <body> after a click on the modal's non-focusable backdrop /
        // header / padding. idx is -1 then; without this guard the forward
        // branch's `idx === length - 1` is false, preventDefault is skipped,
        // and native Tab escapes the modal — here, into the live xterm PTY
        // <textarea> rendered directly behind it.
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

  const onBackdropClick = (e: MouseEvent) => {
    if (e.target === e.currentTarget) props.onCancel();
  };

  return (
    <div
      class="quit-confirm-backdrop"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="brief-clean-title"
      aria-describedby="brief-clean-body"
      onClick={onBackdropClick}
    >
      <div class="quit-confirm-modal">
        <h2 id="brief-clean-title" class="quit-confirm-title">Clean BRIEF?</h2>
        <p id="brief-clean-body" class="quit-confirm-body">
          This <strong>resets</strong> the workgroup BRIEF.md — all frontmatter fields
          and body content are replaced with <code>title: 'Clean'</code> and body
          <code>Ready to start a new topic</code>. If a BRIEF.md exists, a timestamped
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

export default BriefCleanConfirmModal;
