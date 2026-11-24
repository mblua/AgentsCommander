import { Component, onCleanup } from "solid-js";
import { automationAttrs } from "../../shared/automation-hooks";
import { focusOnMount } from "../../shared/focus-on-mount";

/**
 * Confirmation before a replica is unleashed. Escape handling and classes
 * from AgentMatrixNoticeModal; the backdrop button and Escape cancel. Cancel has the
 * default focus, so a stray Enter never unleashes the agent.
 */
const UnleashConfirmModal: Component<{
  replicaName: string;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}> = (props) => {
  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onCancel();
  };
  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  return (
    <div class="modal-overlay">
      {/* The backdrop is a sibling button, so a click inside the dialog never reaches it. */}
      <button
        type="button"
        tabIndex={-1}
        aria-label="Cancel"
        onClick={props.onCancel}
        style={{
          position: "absolute",
          inset: "0",
          background: "transparent",
          border: "none",
          padding: "0",
          cursor: "default",
        }}
        {...automationAttrs("unleashConfirm.overlay", "overlay")}
      />
      <dialog
        open
        class="agent-modal unleash-confirm"
        aria-modal="true"
        aria-labelledby="unleashConfirmTitle"
        aria-describedby="unleashConfirmDescription"
        style={{
          position: "relative",
          margin: "0",
          padding: "0",
          color: "inherit",
          "max-width": "380px",
        }}
        {...automationAttrs("unleashConfirm.modal", "dialog")}
      >
        <div class="agent-modal-header">
          <span id="unleashConfirmTitle" class="agent-modal-title">
            Unleash {props.replicaName}?
          </span>
        </div>
        <div class="new-agent-form" id="unleashConfirmDescription">
          <p style={{ margin: "0", "line-height": "1.5" }}>
            AgentsCommander will remove the workspace restrictions from the context it gives
            this agent. Nothing in its context will try to stop it from changing files outside
            its replica, room or repos.
          </p>
          <p style={{ margin: "0", "line-height": "1.5" }}>
            The Git brakes are removed too: no GIT_CEILING_DIRECTORIES and no git guard on PATH.
          </p>
          <p style={{ margin: "0", "line-height": "1.5" }}>The session will be restarted now.</p>
        </div>
        <div class="new-agent-footer">
          <button
            class="new-agent-cancel-btn"
            autofocus
            ref={(el) => focusOnMount(el)}
            onClick={props.onCancel}
            {...automationAttrs("unleashConfirm.cancel", "button")}
          >
            Cancel
          </button>
          <button
            class="new-agent-create-btn"
            disabled={props.busy}
            onClick={props.onConfirm}
            {...automationAttrs("unleashConfirm.confirm", "button")}
          >
            Unleash and restart
          </button>
        </div>
      </dialog>
    </div>
  );
};

export default UnleashConfirmModal;
