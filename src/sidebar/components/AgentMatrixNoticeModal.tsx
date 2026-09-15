import { Component, onCleanup } from "solid-js";
import { automationAttrs } from "../../shared/automation-hooks";

/**
 * #2046 - left click on an Agent Matrix row. The row is not launchable: this
 * notice says what the matrix is, what a replica is, and where the matrix
 * folder lives. Informational only; it closes on Got it, the overlay click, or
 * Escape, and never starts anything.
 */
const AgentMatrixNoticeModal: Component<{
  name: string;
  path: string;
  onClose: () => void;
}> = (props) => {
  // Registered while the modal is mounted (NewWorkgroupModal precedent,
  // NewWorkgroupModal.tsx:48-52) and removed on unmount.
  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };
  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  // Same display name the row shows (ProjectPanel.tsx:3262-3264).
  const displayName = () => props.name.slice(props.name.lastIndexOf("/") + 1);

  return (
    <div
      class="modal-overlay"
      onClick={props.onClose}
      {...automationAttrs("agentMatrixNotice.overlay", "overlay")}
    >
      <div
        class="agent-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="agentMatrixNoticeTitle"
        aria-describedby="agentMatrixNoticeDescription"
        style={{ "max-width": "380px" }}
        onClick={(e) => e.stopPropagation()}
        {...automationAttrs("agentMatrixNotice.modal", "dialog")}
      >
        <div class="agent-modal-header">
          <span id="agentMatrixNoticeTitle" class="agent-modal-title">
            Agent Matrix
          </span>
        </div>
        <div class="new-agent-form" id="agentMatrixNoticeDescription">
          <p style={{ margin: "0", "line-height": "1.5" }}>
            <strong>{displayName()}</strong> is an Agent Matrix: the canonical definition of
            this agent, with its Role, memory, plans and skills. It is not a session, so it is
            never launched.
          </p>
          <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
            What gets launched are replicas: instances of this agent, inside a team, assigned
            to a room. Start a room's replica from its row.
          </p>
          <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
            The matrix structure is still reachable from this row: right-click it and choose{" "}
            <strong>Open Matrix folder</strong>.
          </p>
          <div class="context-template-path" title={props.path}>
            {props.path}
          </div>
        </div>
        <div class="new-agent-footer">
          <button
            class="new-agent-create-btn"
            autofocus
            onClick={props.onClose}
            {...automationAttrs("agentMatrixNotice.close", "button")}
          >
            Got it
          </button>
        </div>
      </div>
    </div>
  );
};

export default AgentMatrixNoticeModal;
