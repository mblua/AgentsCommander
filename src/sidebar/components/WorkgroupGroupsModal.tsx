import { Component, For, Index, Show, createMemo, createSignal } from "solid-js";
import { focusOnMount } from "../../shared/focus-on-mount";
import type { WorkgroupGroupsConfig } from "../../shared/types";
import {
  MAX_GROUP_NAME_LENGTH,
  MAX_GROUP_REGEX_LENGTH,
  createGroupId,
  exactGroupRegexForWorkgroup,
  validateGroupsConfig,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";

interface WorkgroupGroupsModalProps {
  projectPath: string;
  projectName: string;
  initialWorkgroupName?: string;
  onClose: () => void;
}

function cloneConfig(config: WorkgroupGroupsConfig): WorkgroupGroupsConfig {
  return {
    groups: config.groups.map((group) => ({ ...group })),
    showAll: config.showAll,
    showUngrouped: config.showUngrouped,
  };
}

function nextGroupName(config: WorkgroupGroupsConfig): string {
  const used = new Set(config.groups.map((group) => group.name.trim().toLowerCase()));
  let index = config.groups.length + 1;
  while (used.has(`group ${index}`)) index++;
  return `Group ${index}`;
}

const WorkgroupGroupsModal: Component<WorkgroupGroupsModalProps> = (props) => {
  const [draft, setDraft] = createSignal<WorkgroupGroupsConfig>(
    cloneConfig(workgroupGroupsStore.config(props.projectPath))
  );
  const [saveError, setSaveError] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);

  const validationErrors = createMemo(() =>
    validateGroupsConfig(draft(), { validateRegexSyntax: true })
  );
  const canSave = () => !saving() && validationErrors().length === 0;
  const errorText = () => saveError() ?? workgroupGroupsStore.error(props.projectPath);

  const updateGroup = (id: string, patch: { name?: string; regex?: string }) => {
    setDraft((current) => ({
      ...current,
      groups: current.groups.map((group) =>
        group.id === id ? { ...group, ...patch } : group
      ),
    }));
    setSaveError(null);
  };

  // #746 — id of the row just added via "Add group"; its name input's ref
  // consumes this to focus+select the pre-filled name so the user can type
  // over it immediately. Plain variable (not a signal): addGroup sets it
  // synchronously right before setDraft creates the new row.
  let pendingFocusGroupId: string | null = null;

  const addGroup = () => {
    const current = draft();
    const existingIds = new Set(current.groups.map((group) => group.id));
    const id = createGroupId(existingIds);
    pendingFocusGroupId = id;
    setDraft({
      ...current,
      groups: [
        ...current.groups,
        {
          id,
          name: nextGroupName(current),
          regex: props.initialWorkgroupName
            ? exactGroupRegexForWorkgroup(props.initialWorkgroupName)
            : "(?!)",
        },
      ],
    });
    setSaveError(null);
  };

  const deleteGroup = (id: string) => {
    setDraft((current) => ({
      ...current,
      groups: current.groups.filter((group) => group.id !== id),
    }));
    setSaveError(null);
  };

  const setToggle = (
    field: "showAll" | "showUngrouped",
    value: boolean,
    input: HTMLInputElement
  ) => {
    let accepted = false;
    let previous = input.checked;
    setDraft((current) => {
      previous = current[field];
      const next = { ...current, [field]: value };
      if (!next.showAll && !next.showUngrouped) return current;
      accepted = true;
      return next;
    });
    if (!accepted) input.checked = previous;
    setSaveError(null);
  };

  const save = async () => {
    if (!canSave()) return;
    setSaving(true);
    setSaveError(null);
    try {
      await workgroupGroupsStore.save(props.projectPath, draft());
      props.onClose();
    } catch (error) {
      setSaveError(error instanceof Error ? error.message : String(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div class="modal-overlay" data-ac-testid="workgroupGroups.modal">
      <div class="agent-modal workgroup-groups-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">Edit groups</span>
          <button class="modal-close" onClick={props.onClose} aria-label="Close groups editor">
            &times;
          </button>
        </div>

        <div class="workgroup-groups-form">
          <div class="workgroup-groups-project">{props.projectName}</div>

          <div class="workgroup-groups-toggles">
            <label class="settings-checkbox-field">
              <input
                type="checkbox"
                checked={draft().showAll}
                onChange={(e) => setToggle("showAll", e.currentTarget.checked, e.currentTarget)}
                data-ac-testid="workgroupGroups.toggle.showAll"
              />
              <span>Show All</span>
            </label>
            <label class="settings-checkbox-field">
              <input
                type="checkbox"
                checked={draft().showUngrouped}
                onChange={(e) => setToggle("showUngrouped", e.currentTarget.checked, e.currentTarget)}
                data-ac-testid="workgroupGroups.toggle.showUngrouped"
              />
              <span>Show Ungrouped</span>
            </label>
          </div>

          <div class="workgroup-groups-table">
            {/* #746 — <Index>, NOT <For>: updateGroup replaces the edited row
                object on every keystroke, and a reference-keyed <For> would
                dispose+recreate the row, dropping focus after one character
                (the #614 trap). Position-keyed rows keep the inputs stable. */}
            <Index each={draft().groups}>
              {(group) => (
                <div class="workgroup-group-edit-row" data-ac-testid={`workgroupGroups.row.${group().id}`}>
                  <input
                    class="workgroup-group-name-input"
                    ref={(el) => {
                      if (group().id !== pendingFocusGroupId) return;
                      pendingFocusGroupId = null;
                      focusOnMount(el, { select: true });
                    }}
                    value={group().name}
                    maxLength={MAX_GROUP_NAME_LENGTH}
                    onInput={(e) => updateGroup(group().id, { name: e.currentTarget.value })}
                    aria-label="Group name"
                  />
                  <input
                    class="workgroup-group-regex-input"
                    value={group().regex}
                    maxLength={MAX_GROUP_REGEX_LENGTH}
                    onInput={(e) => updateGroup(group().id, { regex: e.currentTarget.value })}
                    aria-label="Group regex"
                    data-ac-testid={`workgroupGroups.regex.${group().id}`}
                  />
                  <button
                    class="workgroup-group-delete"
                    onClick={() => deleteGroup(group().id)}
                    aria-label={`Delete ${group().name}`}
                  >
                    Delete
                  </button>
                </div>
              )}
            </Index>
            <Show when={draft().groups.length === 0}>
              <div class="workgroup-groups-empty">No groups configured</div>
            </Show>
          </div>

          <button class="workgroup-group-add" onClick={addGroup} data-ac-testid="workgroupGroups.add">
            Add group
          </button>

          <Show when={validationErrors().length > 0}>
            <div class="workgroup-groups-errors" role="alert" data-ac-testid="workgroupGroups.validation">
              <For each={validationErrors()}>{(error) => <div>{error}</div>}</For>
            </div>
          </Show>
          <Show when={errorText()}>
            {(message) => (
              <div class="workgroup-groups-errors" role="alert" data-ac-testid="workgroupGroups.error">
                {message()}
              </div>
            )}
          </Show>
        </div>

        <div class="agent-modal-footer">
          <button class="modal-btn modal-btn-cancel" onClick={props.onClose}>
            Cancel
          </button>
          <button
            class="modal-btn modal-btn-save"
            disabled={!canSave()}
            onClick={save}
            data-ac-testid="workgroupGroups.save"
          >
            {saving() ? "Saving..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default WorkgroupGroupsModal;
