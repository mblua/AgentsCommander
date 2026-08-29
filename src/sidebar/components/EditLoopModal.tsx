import { Component, For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import type {
  AcLoopSummary,
  AcWorkgroup,
  LoopConfigDetails,
  LoopUpdateInput,
} from "../../shared/types";
import { LoopAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";
import {
  busyPolicyForEdit,
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

const EditLoopModal: Component<{
  projectPath: string;
  workgroups: AcWorkgroup[];
  loop: AcLoopSummary;
  onClose: () => void;
}> = (props) => {
  const coordinatorOptions = createMemo(() => coordinatorOptionsFromWorkgroups(props.workgroups));
  const [name, setName] = createSignal(props.loop.name);
  const [expr, setExpr] = createSignal(props.loop.expr);
  const [selectedWorkgroup, setSelectedWorkgroup] = createSignal(props.loop.workgroup);
  const [promptBody, setPromptBody] = createSignal("");
  const [enabled, setEnabled] = createSignal(props.loop.enabled);
  const [forceInject, setForceInject] = createSignal(props.loop.busyCoordinator === "forceInject");
  const [forceCheckboxTouched, setForceCheckboxTouched] = createSignal(false);
  const [loadedDetails, setLoadedDetails] = createSignal<LoopConfigDetails | null>(null);
  const [error, setError] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [loading, setLoading] = createSignal(true);
  const [preview, setPreview] = createSignal<PreviewState>({ status: "idle", message: "" });
  let previewRequestId = 0;
  let mounted = true;

  const loadedBusyPolicy = createMemo(
    () => loadedDetails()?.summary.busyCoordinator ?? props.loop.busyCoordinator
  );

  onMount(async () => {
    try {
      const details = await LoopAPI.getConfig(props.projectPath, props.loop.id);
      if (!mounted) return;
      setLoadedDetails(details);
      setName(details.summary.name);
      setExpr(details.summary.expr);
      setSelectedWorkgroup(details.summary.workgroup);
      setPromptBody(details.promptBody);
      setEnabled(details.summary.enabled);
      setForceInject(details.summary.busyCoordinator === "forceInject");
      setLoading(false);
    } catch (e) {
      if (!mounted) return;
      setError(normalizeLoopError(e, "Failed to load Loop config"));
      setLoading(false);
    }
  });

  onCleanup(() => {
    mounted = false;
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

  const canSave = createMemo(() => {
    const previewState = preview().status;
    return (
      !loading() &&
      name().trim() !== "" &&
      promptBody().trim() !== "" &&
      selectedWorkgroup() !== "" &&
      hasFiveCronFields(expr()) &&
      previewState === "ready" &&
      coordinatorOptions().length > 0
    );
  });

  const handleSave = async () => {
    if (!canSave() || saving()) return;
    setSaving(true);
    setError("");
    try {
      const baseline = loadedDetails();
      if (!baseline) {
        setError("Loop config is still loading");
        setSaving(false);
        return;
      }

      const nextBusyPolicy = busyPolicyForEdit(
        baseline.summary.busyCoordinator,
        forceInject(),
        forceCheckboxTouched()
      );
      const input: LoopUpdateInput = {};
      const nextName = name().trim();
      const nextExpr = expr().trim();

      if (nextName !== baseline.summary.name) input.name = nextName;
      if (nextExpr !== baseline.summary.expr) input.expr = nextExpr;
      if (selectedWorkgroup() !== baseline.summary.workgroup) input.workgroup = selectedWorkgroup();
      if (promptBody() !== baseline.promptBody) input.promptBody = promptBody();
      if (nextBusyPolicy !== baseline.summary.busyCoordinator) input.busyCoordinator = nextBusyPolicy;
      if (enabled() !== baseline.summary.enabled) input.enabled = enabled();

      await LoopAPI.update(props.projectPath, props.loop.id, input);
      await projectStore.reloadProject(props.projectPath);
      props.onClose();
    } catch (e) {
      console.error("update_loop failed:", e);
      setError(normalizeLoopError(e, "Failed to update Loop"));
      setSaving(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !e.isComposing) {
      e.preventDefault();
      void handleSave();
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
          <span class="agent-modal-title">Edit Loop</span>
        </div>

        <div class="new-agent-form">
          <div class="new-agent-field">
            <label class="new-agent-label">ID</label>
            <input
              type="text"
              class="entity-input"
              value={props.loop.id}
              disabled
              data-ac-testid="loop.edit.id"
            />
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Name</label>
            <input
              type="text"
              class="entity-input"
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
              disabled={loading()}
              data-ac-testid="loop.edit.name"
            />
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Cron</label>
            <input
              type="text"
              class="entity-input"
              value={expr()}
              onInput={(e) => setExpr(e.currentTarget.value)}
              disabled={loading()}
              data-ac-testid="loop.edit.cron"
            />
            <Show when={preview().message}>
              <div
                class="loop-preview"
                classList={{ "loop-preview-error": preview().status === "error" }}
                data-ac-testid="loop.edit.cronPreview"
              >
                {preview().message}
              </div>
            </Show>
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Room Orchestrator</label>
            <select
              class="entity-select"
              value={selectedWorkgroup()}
              onChange={(e) => setSelectedWorkgroup(e.currentTarget.value)}
              disabled={loading() || coordinatorOptions().length === 0}
              data-ac-testid="loop.edit.workgroup"
            >
              <option value="" disabled>Select an orchestrator...</option>
              <For each={coordinatorOptions()}>
                {(option) => <option value={option.workgroup}>{option.label}</option>}
              </For>
            </select>
            <Show when={coordinatorOptions().length === 0}>
              <div class="new-agent-error">A room with a verified orchestrator is required.</div>
            </Show>
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Prompt</label>
            <textarea
              class="entity-textarea loop-prompt-textarea"
              value={promptBody()}
              onInput={(e) => setPromptBody(e.currentTarget.value)}
              disabled={loading()}
              data-ac-testid="loop.edit.prompt"
            />
            <div class="entity-textarea-meta">
              <span class="entity-textarea-hint">Ctrl+Enter to save</span>
            </div>
          </div>

          <label class="loop-checkbox-field">
            <input
              type="checkbox"
              checked={enabled()}
              onChange={(e) => setEnabled(e.currentTarget.checked)}
              disabled={loading()}
              data-ac-testid="loop.edit.enabled"
            />
            Enabled
          </label>

          <label class="loop-checkbox-field">
            <input
              type="checkbox"
              checked={forceInject()}
              onChange={(e) => {
                setForceCheckboxTouched(true);
                setForceInject(e.currentTarget.checked);
              }}
              disabled={loading()}
              data-ac-testid="loop.edit.forceInject"
            />
            Force inject even if orchestrator is busy
          </label>

          <Show when={!loading() && loadedBusyPolicy() === "skip" && !forceCheckboxTouched()}>
            <div class="loop-preview">
              Existing busy policy is skip. Saving without changing the checkbox preserves it.
            </div>
          </Show>

          <Show when={loading()}>
            <div class="wizard-loading">Loading Loop...</div>
          </Show>

          <Show when={saving()}>
            <div class="wizard-loading">Saving Loop...</div>
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
            disabled={saving()}
            data-ac-testid="loop.edit.cancel"
          >
            Cancel
          </button>
          <button
            class="new-agent-create-btn"
            disabled={!canSave() || saving()}
            onClick={handleSave}
            data-ac-testid="loop.edit.save"
          >
            {saving() ? "Saving..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default EditLoopModal;
