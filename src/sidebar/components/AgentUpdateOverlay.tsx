import {
  Component,
  For,
  Index,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import { Portal } from "solid-js/web";
import { AgentUpdateAPI } from "../../shared/ipc";
import { toastStore } from "../../shared/stores/toasts";
import { agentUpdateStore, dismissAgentUpdateSummary, markPromptClosed } from "../agent-update";
import { deriveTimelineHeader, deriveTimelineNodes } from "../agent-update-status";

/** #1551 - inline glyphs of the timeline markers and the summary ring (no external asset). */
const CheckIcon: Component = () => (
  <svg
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="2.2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d="M3.3 8.3l3 3 6.4-6.6" />
  </svg>
);

const CrossIcon: Component = () => (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" aria-hidden="true">
    <path d="M4.7 4.7l6.6 6.6M11.3 4.7L4.7 11.3" />
  </svg>
);

/**
 * #1327 - startup coding-agent update overlay.
 *
 * While the backend runs the per-command update sequence: a full-backdrop splash
 * that blocks interaction. While a first-time question is pending: a SI/NO prompt
 * modal on the same overlay. Default = No: Enter/Esc answer No, and No is
 * auto-focused.
 *
 * #1551 round 5 - the splash is the Option 2 staged-progress card (plan 5.13): a
 * header with a small spinner, the title, the `n de N completados` counter and a
 * progress bar; below it a timeline with one node per agent of the pass in pass
 * order (`Pendiente`, `Actualizando...` with its exact update command(s), `Listo`
 * with the version transition, `Falló · <reason>` on one line). When the pass ends
 * with at least one result the card becomes the final summary (green ring, title
 * `Coding agents actualizados`) and stays until the user closes it with `Cerrar`,
 * Enter or Escape on this surface; the failure toasts appear after the close.
 *
 * ONE in-flight flag (`answering`) gates BOTH the buttons AND the Enter/Esc
 * handlers: a keypress during the answer IPC must not fire a second
 * `agent_update_answer` that flips a just-made SI to No. `applied === false` means
 * the answer was late (timeout) or superseded by another surface (#1551 R3-M1):
 * the toast then states the policy the backend recorded, read from the snapshot.
 */
const AgentUpdateOverlay: Component = () => {
  const [state, setState] = agentUpdateStore;
  const [answering, setAnswering] = createSignal(false);
  let closeButton: HTMLButtonElement | undefined;

  const nodeViews = createMemo(() =>
    deriveTimelineNodes(state.nodes, state.running, state.results, state.installAfter)
  );
  const header = createMemo(() => deriveTimelineHeader(nodeViews()));
  const title = () =>
    state.summary === "shown" ? "Coding agents actualizados" : "Actualizando coding agents...";
  const overlayState = () =>
    state.prompt ? "prompt" : state.summary === "shown" ? "summary" : "pass";

  const answer = async (enabled: boolean) => {
    const prompt = state.prompt;
    if (!prompt || answering()) return;
    // #1551 P7 - captured now: `prompt` is a store proxy whose fields a prompt B that
    // arrives during the await overwrites in place (a Solid path write merges into the
    // held object), and the answered command must stay A for the clear and the mark.
    const command = prompt.command;
    setAnswering(true);
    try {
      const applied = await AgentUpdateAPI.answer(command, enabled);
      // P7: the local clear applies only while the answered prompt is still the shown
      // one; the answered command is always marked closed (either outcome), so an
      // older snapshot cannot revive it.
      if (state.prompt?.command === command) setState("prompt", null);
      markPromptClosed(command);
      if (!applied) {
        // R3-M1: a `false` no longer implies "persisted" (late OR superseded): the toast
        // states the policy the backend actually recorded, read from the snapshot.
        try {
          const status = await AgentUpdateAPI.getStatus();
          const persisted = status?.answered?.[command];
          if (persisted === true) {
            toastStore.info("Se actualizará en el próximo arranque.", { durationMs: 10_000 });
          } else if (persisted === false) {
            toastStore.info("No se volverá a preguntar.", { durationMs: 10_000 });
          }
        } catch (err) {
          console.error("[agent-update] getStatus after answer failed:", err);
        }
      }
    } catch (err) {
      // IPC failure: keep the modal open for retry; the backend prompt waits
      // up to 60s.
      toastStore.error(
        err instanceof Error ? err.message : String(err),
        { durationMs: null }
      );
    } finally {
      setAnswering(false);
    }
  };

  onMount(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Enter" && event.key !== "Escape") return;
      if (state.prompt) {
        event.preventDefault();
        event.stopImmediatePropagation();
        if (answering()) return; // R2: no second answer while in flight
        void answer(false); // default = No (Enter and Esc both)
        return;
      }
      // #1551 - the summary closes on Enter/Escape; the capturing handler sees the
      // keydown first, so Enter never activates the button natively (one close, one
      // set of toasts: `dismissAgentUpdateSummary` is inert once dismissed).
      if (state.summary === "shown") {
        event.preventDefault();
        event.stopImmediatePropagation();
        dismissAgentUpdateSummary();
      }
    };
    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => document.removeEventListener("keydown", onKeyDown, true));
  });

  // #1551 - the summary moves focus to `Cerrar`.
  createEffect(() => {
    if (state.summary === "shown") closeButton?.focus();
  });

  return (
    <Show when={state.inProgress || state.prompt || state.summary === "shown"}>
      <Portal>
        <div
          class="agent-update-overlay"
          data-ac-testid="agent-update.overlay"
          data-ac-state={overlayState()}
        >
          <div class="agent-update-card" classList={{ "agent-update-card--timeline": !state.prompt }}>
            <Show when={!state.prompt}>
              <div class="agent-update-header" data-ac-testid="agent-update.header">
                <Show when={state.summary !== "shown"}>
                  <div class="agent-update-spinner agent-update-spinner--small" aria-hidden="true" />
                </Show>
                <Show when={state.summary === "shown"}>
                  <div
                    class="agent-update-done agent-update-done--small"
                    aria-hidden="true"
                    data-ac-testid="agent-update.done"
                  >
                    <CheckIcon />
                  </div>
                </Show>
                <div class="agent-update-header-text">
                  <div
                    class="agent-update-text"
                    role="status"
                    aria-live="polite"
                    data-ac-testid="agent-update.title"
                  >
                    {title()}
                  </div>
                  <Show when={header().total > 0}>
                    <div
                      class="agent-update-progress-text"
                      data-ac-testid="agent-update.progress.text"
                      data-ac-done={header().done}
                      data-ac-total={header().total}
                      data-ac-failed={header().failed}
                    >
                      {header().text}
                    </div>
                  </Show>
                </div>
              </div>
              <Show when={header().total > 0}>
                <div
                  class="agent-update-progress"
                  role="progressbar"
                  aria-label="Agentes completados"
                  aria-valuemin="0"
                  aria-valuemax={header().total}
                  aria-valuenow={header().done}
                  data-ac-testid="agent-update.progress"
                >
                  <div class="agent-update-progress-fill" style={{ width: `${header().percent}%` }} />
                </div>
                {/* <Index> by position: the derivations return new objects on every event; with <For>
                    every <li> would be recreated and the indeterminate bar and the spinning marker
                    would restart. The DOM per position is stable and only attributes and text change. */}
                <ol
                  class="agent-update-timeline"
                  aria-label="Agentes de la actualización"
                  data-ac-testid="agent-update.timeline"
                  data-ac-role="list"
                >
                  <Index each={nodeViews()}>
                    {(node) => (
                      <li
                        class="agent-update-node"
                        data-ac-testid={`agent-update.node.${node().command}`}
                        data-ac-role="listitem"
                        data-ac-state={node().state}
                        data-ac-command={node().command}
                        title={
                          node().state === "updating"
                            ? undefined
                            : node().updateCommands.join("\n") || undefined
                        }
                      >
                        <span class="agent-update-node-marker" aria-hidden="true">
                          {node().state === "ok" ? (
                            <CheckIcon />
                          ) : node().state === "failed" ? (
                            <CrossIcon />
                          ) : null}
                        </span>
                        <div class="agent-update-node-body">
                          <div class="agent-update-node-label">{node().label}</div>
                          <div class="agent-update-node-line">
                            <span
                              class="agent-update-node-state"
                              data-ac-testid={`agent-update.node.${node().command}.state`}
                            >
                              {node().stateText}
                            </span>
                            <Show when={node().detail}>
                              <span class="agent-update-node-sep" aria-hidden="true">
                                {" · "}
                              </span>
                              <span
                                class="agent-update-node-detail"
                                data-ac-testid={`agent-update.node.${node().command}.detail`}
                                title={node().detailTitle ?? undefined}
                              >
                                {node().detail}
                              </span>
                            </Show>
                          </div>
                          <Show when={node().state === "updating"}>
                            <div
                              class="agent-update-node-command"
                              data-ac-testid={`agent-update.node.${node().command}.command`}
                            >
                              <For each={node().updateCommands}>{(step) => <code>{step}</code>}</For>
                            </div>
                            <div class="agent-update-node-bar" aria-hidden="true" />
                          </Show>
                        </div>
                      </li>
                    )}
                  </Index>
                </ol>
              </Show>
              <Show when={state.summary === "shown"}>
                <div class="agent-update-summary-actions">
                  <button
                    ref={closeButton}
                    class="modal-btn modal-btn-save"
                    onClick={() => dismissAgentUpdateSummary()}
                    data-ac-testid="agent-update.summary.close"
                  >
                    Cerrar
                  </button>
                </div>
              </Show>
            </Show>
            <Show when={state.prompt}>
              {(prompt) => (
                <>
                  <div class="agent-update-text agent-update-prompt-text">
                    ¿Querés que al arranque se intente actualizar automáticamente
                    el coding agent {prompt().label}?
                  </div>
                  <div class="agent-update-prompt-actions">
                    <button
                      class="modal-btn modal-btn-save"
                      disabled={answering()}
                      onClick={() => void answer(true)}
                      data-ac-testid="agent-update.prompt.yes"
                    >
                      Sí
                    </button>
                    <button
                      class="modal-btn modal-btn-cancel"
                      autofocus
                      disabled={answering()}
                      onClick={() => void answer(false)}
                      data-ac-testid="agent-update.prompt.no"
                    >
                      No
                    </button>
                  </div>
                </>
              )}
            </Show>
          </div>
        </div>
      </Portal>
    </Show>
  );
};

export default AgentUpdateOverlay;
