import { Component } from "solid-js";
import { automationAttrs } from "../../shared/automation-hooks";

/**
 * #537: Post-assign confirmation. After a Coding Agent is assigned to a running WG
 * replica (replica scope), the selection is persisted but the live session keeps the
 * old agent until it restarts. This lite modal offers that restart immediately so the
 * assignment applies without a manual restart.
 *
 * Presentational only: the caller decides when to show it (a live session must exist)
 * and supplies the restart action. Mirrors the Delete-Workgroup modal shell
 * (`modal-overlay` + `agent-modal` + `new-agent-footer`).
 */
const RestartPromptModal: Component<{
  agentLabel: string;
  replicaName: string;
  onRestart: () => void;
  onLater: () => void;
}> = (props) => {
  return (
    <div
      class="modal-overlay"
      onClick={props.onLater}
      {...automationAttrs("restartPrompt.overlay", "overlay")}
    >
      <div
        class="agent-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="restartPromptTitle"
        style={{ "max-width": "360px" }}
        onClick={(e) => e.stopPropagation()}
        {...automationAttrs("restartPrompt.modal", "dialog")}
      >
        <div class="agent-modal-header">
          <span id="restartPromptTitle" class="agent-modal-title">Apply Coding Agent?</span>
        </div>
        <div class="new-agent-form">
          <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
            <strong>{props.agentLabel}</strong> assigned to <strong>{props.replicaName}</strong>.
            Restart the session now to apply it?
          </p>
          <div class="new-agent-footer">
            <button
              class="new-agent-cancel-btn"
              onClick={props.onLater}
              {...automationAttrs("restartPrompt.later", "button")}
            >
              Later
            </button>
            <button
              class="new-agent-create-btn"
              onClick={props.onRestart}
              {...automationAttrs("restartPrompt.restart", "button")}
            >
              Restart now
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

export default RestartPromptModal;
