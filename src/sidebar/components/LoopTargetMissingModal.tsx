import { Component, For, Show, onCleanup } from "solid-js";
import { automationAttrs } from "../../shared/automation-hooks";
import type { UnresolvedLoopTarget } from "../../shared/types";

/**
 * #2171 - startup notice for Loops whose configured room cannot be resolved in
 * this project. Presentational only: it holds no state and performs no IPC; the
 * owner (ProjectPanel) loads the alerts, opens the Loop configuration and keeps
 * the per-row errors.
 */
const LoopTargetMissingModal: Component<{
  alerts: UnresolvedLoopTarget[];
  openErrors: Record<string, string>;
  busyLoopId: string | null;
  onOpenConfig: (alert: UnresolvedLoopTarget) => void;
  onDismiss: () => void;
}> = (props) => {
  // Escape dismisses from anywhere: the document listener covers the case where
  // nothing inside the modal has focus, the element handlers give the overlay and
  // the dialog their own keyboard path. One key press dismisses once, whichever
  // listener the browser runs first.
  const handledEscapes = new WeakSet<KeyboardEvent>();
  const dismissOnEscape = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || handledEscapes.has(e)) return;
    handledEscapes.add(e);
    props.onDismiss();
  };
  // Registered while the modal is mounted (AgentMatrixNoticeModal precedent)
  // and removed on unmount.
  document.addEventListener("keydown", dismissOnEscape);
  onCleanup(() => document.removeEventListener("keydown", dismissOnEscape));

  return (
    <div
      class="modal-overlay"
      onClick={props.onDismiss}
      onKeyDown={dismissOnEscape}
      {...automationAttrs("loopTargetMissing.overlay", "overlay")}
    >
      <div
        class="agent-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="loopTargetMissingTitle"
        aria-describedby="loopTargetMissingDescription"
        style={{ "max-width": "520px" }}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={dismissOnEscape}
        {...automationAttrs("loopTargetMissing.modal", "dialog")}
      >
        <div class="agent-modal-header">
          <span id="loopTargetMissingTitle" class="agent-modal-title">
            Loop target not found
          </span>
        </div>
        <div class="new-agent-form" id="loopTargetMissingDescription">
          <p style={{ margin: "0", "line-height": "1.5" }}>
            These Loops point at a room that does not exist in this project, so they cannot
            run.
          </p>
          <For each={props.alerts}>
            {(alert) => (
              <div
                style={{
                  display: "flex",
                  "flex-direction": "column",
                  gap: "6px",
                  "line-height": "1.5",
                }}
              >
                <strong>{alert.loopName}</strong>
                <span style={{ opacity: 0.85 }}>Room: {alert.workgroup}</span>
                <div class="context-template-path" title={alert.projectPath}>
                  {alert.projectPath}
                </div>
                <span style={{ opacity: 0.85 }}>{alert.error}</span>
                <button
                  class="new-agent-create-btn"
                  disabled={props.busyLoopId === alert.loopId}
                  onClick={() => props.onOpenConfig(alert)}
                  {...automationAttrs(`loopTargetMissing.open.${alert.loopId}`, "button")}
                >
                  Open Loop configuration
                </button>
                <Show when={props.openErrors[alert.loopId]}>
                  {(message) => (
                    <span class="new-agent-error" style={{ "line-height": "1.5" }}>
                      {message()}
                    </span>
                  )}
                </Show>
              </div>
            )}
          </For>
        </div>
        <div class="new-agent-footer">
          <button
            class="new-agent-create-btn"
            onClick={props.onDismiss}
            {...automationAttrs("loopTargetMissing.dismiss", "button")}
          >
            Dismiss
          </button>
        </div>
      </div>
    </div>
  );
};

export default LoopTargetMissingModal;
