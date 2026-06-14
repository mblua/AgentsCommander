import { Component, For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { AcWorkgroup } from "../../shared/types";
import { LoopAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";
import {
  busyPolicyFromForceCheckbox,
  coordinatorOptionsFromWorkgroups,
  formatLoopNextDue,
  hasFiveCronFields,
  normalizeLoopError,
} from "./loop-modal-helpers";

type PreviewState =
  | { status: "idle"; message: string }
  | { status: "loading"; message: string }
  | { status: "ready"; message: string }
  | { status: "error"; message: string };

const NewLoopModal: Component<{
  projectPath: string;
  workgroups: AcWorkgroup[];
  onClose: () => void;
}> = (props) => {
  const coordinatorOptions = createMemo(() => coordinatorOptionsFromWorkgroups(props.workgroups));
  const [name, setName] = createSignal("");
  const [expr, setExpr] = createSignal("");
  const [selectedWorkgroup, setSelectedWorkgroup] = createSignal("");
  const [promptBody, setPromptBody] = createSignal("");
  const [enabled, setEnabled] = createSignal(true);
  const [forceInject, setForceInject] = createSignal(false);
  const [error, setError] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  const [preview, setPreview] = createSignal<PreviewState>({ status: "idle", message: "" });
  let previewRequestId = 0;

  createEffect(() => {
    const options = coordinatorOptions();
    if (!selectedWorkgroup() && options.length === 1) {
      setSelectedWorkgroup(options[0].workgroup);
    }
  });

  createEffect(() => {
    const value = expr().trim();
    const requestId = ++previewRequestId;
    if (!value) {
      setPreview({ status: "idle", message: "" });
      return;
    }
    if (!hasFiveCronFields(value)) {
      setPreview({
        status: "error",
        message: "Cron expression must have exactly five fields",
      });
      return;
    }

    setPreview({ status: "loading", message: "Checking schedule..." });
    const timer = window.setTimeout(async () => {
      try {
        const result = await LoopAPI.previewCron(value);
        if (requestId !== previewRequestId) return;
        const formatted = formatLoopNextDue(result.nextDueAt);
        setPreview({
          status: "ready",
          message: formatted ? `Next run: ${formatted}` : "No upcoming run found",
        });
      } catch (e) {
        if (requestId !== previewRequestId) return;
        setPreview({
          status: "error",
          message: normalizeLoopError(e, "Invalid cron expression"),
        });
      }
    }, 300);

    onCleanup(() => window.clearTimeout(timer));
  });

  const canCreate = createMemo(() => {
    const previewState = preview().status;
    return (
      name().trim() !== "" &&
      promptBody().trim() !== "" &&
      selectedWorkgroup() !== "" &&
      hasFiveCronFields(expr()) &&
      previewState === "ready" &&
      coordinatorOptions().length > 0
    );
  });

  const handleCreate = async () => {
    if (!canCreate() || creating()) return;
    setCreating(true);
    setError("");
    try {
      await LoopAPI.create(props.projectPath, {
        name: name().trim(),
        expr: expr().trim(),
        workgroup: selectedWorkgroup(),
        promptBody: promptBody(),
        busyCoordinator: busyPolicyFromForceCheckbox(forceInject()),
        enabled: enabled(),
      });
      await projectStore.reloadProject(props.projectPath);
      props.onClose();
    } catch (e) {
      console.error("create_loop failed:", e);
      setError(normalizeLoopError(e, "Failed to create Loop"));
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !e.isComposing) {
      e.preventDefault();
      void handleCreate();
    }
  };

  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };

  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  return (
    <div class="modal-overlay" onKeyDown={handleKeyDown}>
      <div class="agent-modal new-agent-modal loop-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">New Loop</span>
        </div>

        <div class="new-agent-form">
          <div class="new-agent-field">
            <label class="new-agent-label">Name</label>
            <input
              type="text"
              class="entity-input"
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
              placeholder="Weekday standup"
              autofocus
              data-ac-testid="loop.new.name"
            />
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Cron</label>
            <input
              type="text"
              class="entity-input"
              value={expr()}
              onInput={(e) => setExpr(e.currentTarget.value)}
              placeholder="0 9 * * 1-5"
              data-ac-testid="loop.new.cron"
            />
            <Show when={preview().message}>
              <div
                class="loop-preview"
                classList={{ "loop-preview-error": preview().status === "error" }}
                data-ac-testid="loop.new.cronPreview"
              >
                {preview().message}
              </div>
            </Show>
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Workgroup Coordinator</label>
            <select
              class="entity-select"
              value={selectedWorkgroup()}
              onChange={(e) => setSelectedWorkgroup(e.currentTarget.value)}
              disabled={coordinatorOptions().length === 0}
              data-ac-testid="loop.new.workgroup"
            >
              <option value="" disabled>Select a coordinator...</option>
              <For each={coordinatorOptions()}>
                {(option) => <option value={option.workgroup}>{option.label}</option>}
              </For>
            </select>
            <Show when={coordinatorOptions().length === 0}>
              <div class="new-agent-error">A workgroup with a verified coordinator is required.</div>
            </Show>
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Prompt</label>
            <textarea
              class="entity-textarea loop-prompt-textarea"
              value={promptBody()}
              onInput={(e) => setPromptBody(e.currentTarget.value)}
              placeholder="Prompt to inject into the coordinator"
              data-ac-testid="loop.new.prompt"
            />
            <div class="entity-textarea-meta">
              <span class="entity-textarea-hint">Ctrl+Enter to create</span>
            </div>
          </div>

          <label class="loop-checkbox-field">
            <input
              type="checkbox"
              checked={enabled()}
              onChange={(e) => setEnabled(e.currentTarget.checked)}
              data-ac-testid="loop.new.enabled"
            />
            Enabled
          </label>

          <label class="loop-checkbox-field">
            <input
              type="checkbox"
              checked={forceInject()}
              onChange={(e) => setForceInject(e.currentTarget.checked)}
              data-ac-testid="loop.new.forceInject"
            />
            Force inject even if coordinator is busy
          </label>

          <Show when={creating()}>
            <div class="wizard-loading">Creating Loop...</div>
          </Show>

          <Show when={error()}>
            <div class="new-agent-error">{error()}</div>
          </Show>
        </div>

        <div class="new-agent-footer">
          <button
            type="button"
            class="new-agent-cancel-btn"
            onClick={() => props.onClose()}
            disabled={creating()}
          >
            Cancel
          </button>
          <button
            class="new-agent-create-btn"
            disabled={!canCreate() || creating()}
            onClick={handleCreate}
            data-ac-testid="loop.new.create"
          >
            {creating() ? "Creating..." : "Create"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default NewLoopModal;
