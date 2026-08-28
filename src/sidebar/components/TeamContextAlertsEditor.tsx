import { createEffect, createMemo, For, onCleanup, Show } from "solid-js";
import type { Component } from "solid-js";
import { focusOnMount } from "../../shared/focus-on-mount";
import type {
  ContextAlertThresholdDraft,
  ContextAlertThresholdValidation,
} from "./team-context-alerts";
import { MAX_CONTEXT_ALERT_THRESHOLDS } from "./team-context-alerts";

interface TeamContextAlertsEditorProps {
  idPrefix: string;
  drafts: readonly ContextAlertThresholdDraft[];
  validation: ContextAlertThresholdValidation;
  disabled: boolean;
  onChange: (next: ContextAlertThresholdDraft[]) => void;
}

export const TeamContextAlertsEditor: Component<TeamContextAlertsEditorProps> = (props) => {
  const inputElements = new Map<number, HTMLInputElement>();
  let addButton: HTMLButtonElement | undefined;
  let focusAfterAddId: number | null = null;

  const draftIds = createMemo(() => props.drafts.map((draft) => draft.id));
  const headingId = () => `${props.idPrefix}-heading`;
  const helpId = () => `${props.idPrefix}-help`;
  const countId = () => `${props.idPrefix}-count`;
  const aggregateErrorId = () => `${props.idPrefix}-errors`;
  const inputId = (draftId: number) => `${props.idPrefix}-threshold-${draftId}`;
  const rowErrorId = (draftId: number) => `${inputId(draftId)}-error`;
  const draftById = (draftId: number) =>
    props.drafts.find((draft) => draft.id === draftId);
  const validationById = (draftId: number) =>
    props.validation.rows.find((row) => row.draftId === draftId);

  createEffect(() => {
    const editorDisabled = props.disabled;
    for (const draft of props.drafts) {
      const input = inputElements.get(draft.id);
      if (input && (editorDisabled || input.value !== draft.raw)) input.value = draft.raw;
    }
  });

  const addThreshold = () => {
    if (props.disabled || props.drafts.length >= MAX_CONTEXT_ALERT_THRESHOLDS) return;
    const id = Math.max(0, ...props.drafts.map(({ id: draftId }) => draftId)) + 1;
    focusAfterAddId = id;
    props.onChange([...props.drafts, { id, raw: "" }]);
  };

  const updateThreshold = (draftId: number, raw: string) => {
    if (props.disabled) return;
    props.onChange(
      props.drafts.map((draft) =>
        draft.id === draftId ? { ...draft, raw } : draft,
      ),
    );
  };

  const removeThreshold = (draftId: number) => {
    if (props.disabled) return;
    const currentIndex = props.drafts.findIndex((draft) => draft.id === draftId);
    if (currentIndex < 0) return;

    const next = props.drafts.filter((draft) => draft.id !== draftId);
    const focusDraft = next[currentIndex] ?? next[currentIndex - 1];
    props.onChange(next);

    const focusTarget = focusDraft ? inputElements.get(focusDraft.id) : addButton;
    if (focusTarget) focusOnMount(focusTarget);
  };

  return (
    <section class="team-settings-section" aria-labelledby={headingId()}>
      <h3 id={headingId()} class="team-settings-heading">
        Context usage alerts (optional)
      </h3>
      <p id={helpId()} class="team-context-alert-help">
        Applies to every room of this team. When a member reaches or crosses a configured
        percentage, AgentsCommander sends that room&apos;s orchestrator an informational notice
        naming the member and observed usage. Readings are best-effort and depend on a working
        contextRegex (Context badge pattern) for the coding agent selected for the session. No
        automatic action is taken: no session is cleared, closed, restarted, handed off,
        reassigned, or otherwise changed.
      </p>

      <div class="team-context-alert-toolbar">
        <button
          ref={addButton}
          class="new-agent-browse-btn"
          type="button"
          disabled={props.disabled || props.drafts.length >= MAX_CONTEXT_ALERT_THRESHOLDS}
          aria-describedby={countId()}
          onClick={addThreshold}
        >
          Add threshold
        </button>
        <span id={countId()} class="team-context-alert-count" aria-live="polite">
          {props.drafts.length} of {MAX_CONTEXT_ALERT_THRESHOLDS} thresholds
        </span>
      </div>

      <Show when={props.drafts.length === 0}>
        <div class="wizard-empty team-context-alert-empty">No context alerts configured.</div>
      </Show>

      <div class="team-context-alert-rows">
        <For each={draftIds()}>
          {(draftId, index) => {
            onCleanup(() => inputElements.delete(draftId));
            const rowValidation = () => validationById(draftId);
            const hasError = () => (rowValidation()?.errorCodes.length ?? 0) > 0;
            const describedBy = () =>
              hasError() ? `${helpId()} ${rowErrorId(draftId)}` : helpId();

            return (
              <div class="team-context-alert-row">
                <div class="team-context-alert-field">
                  <label class="new-agent-label" for={inputId(draftId)}>
                    Threshold {index() + 1} percentage
                  </label>
                  <div class="team-context-alert-input-wrap">
                    <input
                      ref={(element) => {
                        inputElements.set(draftId, element);
                        if (focusAfterAddId === draftId) {
                          focusAfterAddId = null;
                          focusOnMount(element);
                        }
                      }}
                      id={inputId(draftId)}
                      class="agent-search-input team-context-alert-input"
                      type="text"
                      inputmode="numeric"
                      autocomplete="off"
                      value={draftById(draftId)?.raw ?? ""}
                      disabled={props.disabled}
                      aria-describedby={describedBy()}
                      aria-invalid={hasError() ? "true" : undefined}
                      onInput={(event) => {
                        if (props.disabled) {
                          event.currentTarget.value = draftById(draftId)?.raw ?? "";
                          return;
                        }
                        updateThreshold(draftId, event.currentTarget.value);
                      }}
                    />
                    <span class="team-context-alert-suffix" aria-hidden="true">%</span>
                  </div>
                  <Show when={hasError()}>
                    <div id={rowErrorId(draftId)} class="team-context-alert-row-error">
                      {rowValidation()?.messages.join(" ")}
                    </div>
                  </Show>
                </div>
                <button
                  class="new-agent-cancel-btn team-context-alert-remove"
                  type="button"
                  disabled={props.disabled}
                  aria-label={`Remove threshold ${index() + 1}`}
                  onClick={() => removeThreshold(draftId)}
                >
                  Remove
                </button>
              </div>
            );
          }}
        </For>
      </div>

      <Show when={props.validation.summaryMessages.length > 0}>
        <div
          id={aggregateErrorId()}
          class="new-agent-error team-context-alert-errors"
          role="alert"
          aria-label="Context alert threshold errors"
        >
          <For each={props.validation.summaryMessages}>
            {(message) => <div>{message}</div>}
          </For>
        </div>
      </Show>
    </section>
  );
};
