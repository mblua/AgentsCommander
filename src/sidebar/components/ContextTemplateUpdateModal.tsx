import { Component, Show } from "solid-js";
import type { ContextTemplateUpdate } from "../../shared/types";
import { automationAttrs } from "../../shared/automation-hooks";

const ContextTemplateUpdateModal: Component<{
  update: ContextTemplateUpdate;
  busy: boolean;
  error: string | null;
  onKeep: () => void;
  onOverwrite: () => void;
}> = (props) => {
  const swallowEscape = (e: KeyboardEvent) => {
    if (e.key === "Escape") e.stopPropagation();
  };

  return (
    <div
      class="modal-overlay"
      onKeyDown={swallowEscape}
      {...automationAttrs("contextTemplateUpdate.overlay", "overlay")}
    >
      <div
        class="modal-container context-template-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="contextTemplateUpdateTitle"
        {...automationAttrs("contextTemplateUpdate.modal", "dialog")}
      >
        <div class="modal-header">
          <span id="contextTemplateUpdateTitle" class="modal-title">
            Context template update
          </span>
        </div>
        <div class="modal-body">
          <p class="context-template-note">
            A newer built-in default is available for{" "}
            <strong>{props.update.label}</strong>, but your copy of this file
            appears customized, so it was left unchanged.
          </p>
          <div class="context-template-path">{props.update.filePath}</div>
          <p class="context-template-note context-template-note-dim">
            Keep your version, or overwrite it with the new default. Overwriting
            first saves your current file as a <code>.bak</code> backup beside
            it.
          </p>
        </div>
        <div class="modal-footer">
          <Show when={props.error}>
            <div
              class="modal-save-error"
              {...automationAttrs("contextTemplateUpdate.error", "status")}
            >
              {props.error}
            </div>
          </Show>
          <button
            class="modal-btn modal-btn-cancel"
            onClick={props.onKeep}
            disabled={props.busy}
            {...automationAttrs("contextTemplateUpdate.keep", "button")}
          >
            Keep my version
          </button>
          <button
            class="modal-btn modal-btn-save"
            onClick={props.onOverwrite}
            disabled={props.busy}
            {...automationAttrs("contextTemplateUpdate.overwrite", "button")}
          >
            Overwrite with default
          </button>
        </div>
      </div>
    </div>
  );
};

export default ContextTemplateUpdateModal;
