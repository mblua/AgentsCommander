import { Component, Show, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { AgentUpdateAPI } from "../../shared/ipc";
import { toastStore } from "../../shared/stores/toasts";
import { agentUpdateStore } from "../agent-update";

/**
 * #1327 - startup coding-agent update overlay.
 *
 * While the backend runs the per-command update sequence: a full-backdrop splash
 * ("Actualizando coding agents...") that blocks interaction. While a first-time
 * question is pending: a SI/NO prompt modal on the same overlay. Default = No:
 * Enter/Esc answer No, and No is auto-focused.
 *
 * ONE in-flight flag (`answering`) gates BOTH the buttons AND the Enter/Esc
 * handlers: a keypress during the answer IPC must not fire a second
 * `agent_update_answer` that flips a just-made SI to No. With the single flag,
 * `applied === false` reliably means "genuinely late answer" (F4).
 */
const AgentUpdateOverlay: Component = () => {
  const [state, setState] = agentUpdateStore;
  const [answering, setAnswering] = createSignal(false);

  const answer = async (enabled: boolean) => {
    const prompt = state.prompt;
    if (!prompt || answering()) return;
    setAnswering(true);
    try {
      const applied = await AgentUpdateAPI.answer(prompt.command, enabled);
      setState("prompt", null);
      if (!applied) {
        // R2 pin: conditional text is factually correct for both answers.
        toastStore.info(
          enabled
            ? "Se actualizará en el próximo arranque."
            : "No se volverá a preguntar.",
          { durationMs: 10_000 }
        );
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
      if (!state.prompt) return;
      if (event.key !== "Enter" && event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (answering()) return; // R2: no second answer while in flight
      void answer(false); // default = No (Enter and Esc both)
    };
    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => document.removeEventListener("keydown", onKeyDown, true));
  });

  return (
    <Show when={state.inProgress || state.prompt}>
      <Portal>
        <div class="agent-update-overlay" data-ac-testid="agent-update.overlay">
          <div class="agent-update-card">
            <Show when={!state.prompt}>
              <div class="agent-update-spinner" aria-hidden="true" />
              <div class="agent-update-text">Actualizando coding agents...</div>
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
