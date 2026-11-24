// Settings > Appearance > Hotkeys screenshot checkboxes (Hyprland bind, clipboard mode).
// Presentational only; the modal owns the draft and Save/Cancel.
import type { Component } from "solid-js";

interface ScreenshotSettingsProps {
  hyprlandBind: boolean;
  copyPath: boolean;
  onHyprlandBind: (on: boolean) => void;
  onCopyPath: (on: boolean) => void;
}

const ScreenshotSettings: Component<ScreenshotSettingsProps> = (props) => (
  <>
    <label data-ac-setting="screenshotHyprlandBind" class="settings-checkbox-field">
      <input
        type="checkbox"
        class="settings-checkbox"
        checked={props.hyprlandBind}
        onChange={(e) => props.onHyprlandBind(e.currentTarget.checked)}
        data-ac-testid="settings.general.screenshotHyprlandBind"
      />
      <span class="settings-label">Add the screenshot shortcut to Hyprland for me</span>
    </label>
    <div class="settings-hint">
      Linux with Hyprland (Omarchy): AgentsCommander writes its own marked lines in
      ~/.config/hypr/bindings.lua and keeps them in step with the key above. Turn off
      to remove them.
    </div>
    <label data-ac-setting="screenshotCopyPath" class="settings-checkbox-field">
      <input
        type="checkbox"
        class="settings-checkbox"
        checked={props.copyPath}
        onChange={(e) => props.onCopyPath(e.currentTarget.checked)}
        data-ac-testid="settings.general.screenshotCopyPath"
      />
      <span class="settings-label">Paste image path instead of image</span>
    </label>
    <div class="settings-hint">
      Screenshots always save a PNG. Off: the picture goes to the clipboard. On: its file path
      does.
    </div>
  </>
);

export default ScreenshotSettings;
