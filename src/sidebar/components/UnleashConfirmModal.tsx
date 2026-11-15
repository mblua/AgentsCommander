import { Component, onCleanup } from "solid-js";
import { automationAttrs } from "../../shared/automation-hooks";
import { focusOnMount } from "../../shared/focus-on-mount";

/**
 * #4752 - confirmation before a replica is unleashed. Same structure as
 * AgentMatrixNoticeModal: the overlay click and Escape cancel. Cancel has the
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
    <div
      class="modal-overlay"
      onClick={props.onCancel}
      {...automationAttrs("unleashConfirm.overlay", "overlay")}
    >
      <div
        class="agent-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="unleashConfirmTitle"
        aria-describedby="unleashConfirmDescription"
        style={{ "max-width": "380px" }}
        onClick={(e) => e.stopPropagation()}
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
      </div>
    </div>
  );
};

export default UnleashConfirmModal;
