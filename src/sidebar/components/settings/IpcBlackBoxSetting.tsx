// Settings > General > System > Logging: the renderer IPC black box toggle.
// The modal owns the draft and Save; the backend reads the value once at start.
import type { Component } from "solid-js";

export interface IpcBlackBoxSettingProps {
  checked: boolean;
  disabled: boolean;
  onChange: (checked: boolean) => void;
}

const IpcBlackBoxSetting: Component<IpcBlackBoxSettingProps> = (props) => (
  <>
    <label data-ac-setting="ipcBlackBoxEnabled" class="settings-checkbox-field">
      <input
        type="checkbox"
        class="settings-checkbox"
        checked={props.checked}
        disabled={props.disabled}
        onChange={(e) => props.onChange(e.currentTarget.checked)}
        data-ac-testid="settings.general.ipcBlackBoxEnabled"
        data-ac-role="checkbox"
        data-ac-state={props.checked ? "checked" : "unchecked"}
      />
      <span>Record IPC black box (renderer freeze diagnostics)</span>
    </label>
    <div class="settings-hint" data-ac-testid="settings.general.ipcBlackBoxEnabled.hint">
      Records in-flight IPC calls and frame, input and event liveness of every window so
      a freeze can be diagnosed on the next start. Applies on the next restart of
      AgentsCommander; the current session is unaffected. Disabled by default.
    </div>
  </>
);

export default IpcBlackBoxSetting;
