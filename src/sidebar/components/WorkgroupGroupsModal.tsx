import { Component, For, Index, Show, createMemo, createResource, createSignal } from "solid-js";
import { focusOnMount } from "../../shared/focus-on-mount";
import { SettingsAPI } from "../../shared/ipc";
import { isWindows } from "../../shared/platform";
import type { NonStopGroupConfig, WorkgroupGroupsConfig } from "../../shared/types";
import {
  MAX_GROUP_NAME_LENGTH,
  MAX_GROUP_REGEX_LENGTH,
  DEFAULT_NON_STOP_NAME,
  createGroupId,
  defaultNonStop,
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
    // #777 (dev-webpage-ui #1, BLOCKER): the modal seeds its draft with THIS local
    // clone (not the store's), so it MUST deep-clone nonStop too, or opening the
    // modal drops a persisted nonStop and the first Save wipes it.
    nonStop: config.nonStop
      ? { ...config.nonStop, telegram: { ...config.nonStop.telegram }, sound: { ...config.nonStop.sound } }
      : (config.nonStop ?? undefined),
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

  // #777 Non-stop section state. An absent nonStop renders as the default for
  // editing; the first touch materializes it in the draft (persisted by save()).
  const ns = (): NonStopGroupConfig => draft().nonStop ?? defaultNonStop();
  const setNonStop = (patch: Partial<NonStopGroupConfig>) => {
    setDraft({ ...draft(), nonStop: { ...ns(), ...patch } });
    setSaveError(null);
  };
  const setNonStopTelegram = (patch: Partial<NonStopGroupConfig["telegram"]>) => {
    const current = ns();
    setDraft({ ...draft(), nonStop: { ...current, telegram: { ...current.telegram, ...patch } } });
    setSaveError(null);
  };
  const setNonStopSound = (patch: Partial<NonStopGroupConfig["sound"]>) => {
    const current = ns();
    setDraft({ ...draft(), nonStop: { ...current, sound: { ...current.sound, ...patch } } });
    setSaveError(null);
  };
  // Bots reuse the existing settings fetch (Change L dropped). The fetch swallows
  // any error so a failed/absent settings handler yields an empty bot list (no
  // unhandled rejection) rather than surfacing an error in the modal.
  const [botsResource] = createResource(async () => {
    try {
      return await SettingsAPI.get();
    } catch {
      return null;
    }
  });
  const bots = () => botsResource()?.telegramBots ?? [];

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

          {/* #777 Non-stop watchdog section. Static JSX (NOT a keyed <For> over
              field rows) so the text inputs keep focus (the #614 lesson). The
              single show toggle = rail visibility AND watchdog on/off; Telegram
              and Sound are independent measures. */}
          <div class="workgroup-groups-nonstop">
            <label class="settings-checkbox-field">
              <input
                type="checkbox"
                checked={ns().show}
                onChange={(e) => setNonStop({ show: e.currentTarget.checked })}
                data-ac-testid="workgroupGroups.nonstop.show"
              />
              <span>{DEFAULT_NON_STOP_NAME} (watchdog)</span>
            </label>
            <div class="workgroup-group-edit-row">
              <input
                class="workgroup-group-name-input"
                value={ns().name}
                maxLength={MAX_GROUP_NAME_LENGTH}
                onInput={(e) => setNonStop({ name: e.currentTarget.value })}
                aria-label={`${DEFAULT_NON_STOP_NAME} name`}
                data-ac-testid="workgroupGroups.nonstop.name"
              />
              <input
                class="workgroup-group-regex-input"
                value={ns().regex}
                maxLength={MAX_GROUP_REGEX_LENGTH}
                onInput={(e) => setNonStop({ regex: e.currentTarget.value })}
                aria-label={`${DEFAULT_NON_STOP_NAME} regex`}
                data-ac-testid="workgroupGroups.nonstop.regex"
              />
            </div>
            <label class="workgroup-groups-nonstop-field">
              <span>Tolerance (seconds)</span>
              <input
                type="number"
                min="1"
                max="3600"
                value={ns().toleranceSeconds}
                onInput={(e) => setNonStop({ toleranceSeconds: Number(e.currentTarget.value) })}
                data-ac-testid="workgroupGroups.nonstop.tolerance"
              />
            </label>
            <div class="workgroup-groups-nonstop-measure">
              <label class="settings-checkbox-field">
                <input
                  type="checkbox"
                  checked={ns().telegram.enabled}
                  onChange={(e) => setNonStopTelegram({ enabled: e.currentTarget.checked })}
                  data-ac-testid="workgroupGroups.nonstop.telegramEnabled"
                />
                <span>Telegram alert</span>
              </label>
              <select
                class="workgroup-groups-nonstop-bot"
                value={ns().telegram.botId ?? ""}
                disabled={!ns().telegram.enabled}
                onChange={(e) => setNonStopTelegram({ botId: e.currentTarget.value || null })}
                data-ac-testid="workgroupGroups.nonstop.telegramBot"
              >
                <option value="">First configured bot</option>
                <For each={bots()}>
                  {(bot) => <option value={bot.id}>{bot.label}</option>}
                </For>
              </select>
              <Show when={ns().telegram.enabled && bots().length === 0}>
                <div
                  class="workgroup-groups-nonstop-warning"
                  data-ac-testid="workgroupGroups.nonstop.telegramNoBots"
                >
                  No Telegram bots configured; alerts will not fire.
                </div>
              </Show>
            </div>
            <div class="workgroup-groups-nonstop-measure">
              <label class="settings-checkbox-field">
                <input
                  type="checkbox"
                  checked={ns().sound.enabled}
                  disabled={!isWindows}
                  onChange={(e) => setNonStopSound({ enabled: e.currentTarget.checked })}
                  data-ac-testid="workgroupGroups.nonstop.soundEnabled"
                />
                <span>Sound alert</span>
              </label>
              <input
                type="number"
                min="1"
                max="60"
                value={ns().sound.seconds}
                disabled={!isWindows || !ns().sound.enabled}
                onInput={(e) => setNonStopSound({ seconds: Number(e.currentTarget.value) })}
                aria-label="Sound alert seconds"
                data-ac-testid="workgroupGroups.nonstop.soundSeconds"
              />
              <Show when={!isWindows}>
                <div
                  class="workgroup-groups-nonstop-hint"
                  data-ac-testid="workgroupGroups.nonstop.soundWindowsOnly"
                >
                  Sound alert is Windows-only.
                </div>
              </Show>
            </div>
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
