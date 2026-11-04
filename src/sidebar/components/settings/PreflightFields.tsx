// #3516 - the collapsed "Preflight" block of a Coding Agent card or a Profile cell.
// Controlled: the modal owns every value and draft; this block keeps no state.
import { type Component, Show, createUniqueId } from "solid-js";
import { INVALID_TIMEOUT_TEXT } from "./preflightModel";

const LEVEL_HELP = {
  agent: "Runs before every new session for this Coding Agent, before the profile preflight. "
    + "Leave empty to skip this step.",
  profile: "Runs after the Coding Agent preflight for this effective profile. "
    + "Leave empty to skip this step.",
} as const;
const COMMON_HELP = "A finite command or script. Exit code 0 allows launch; any failure blocks "
  + "launch. Uses the session working directory and environment.";
const TIMEOUT_HELP = "Default: 60 seconds. Each preflight has its own timeout. "
  + "Leave this field empty or reset it to use the default.";
const PROFILE_TIMEOUT_HELP = "The effective Profile uses its own timeout, including when "
  + "falling back to Profile A. It does not inherit the Coding Agent timeout.";

export interface PreflightFieldsProps {
  level: "agent" | "profile";
  command: string | undefined;
  timeoutText: string;
  timeoutInvalid: boolean;
  onCommand: (value: string) => void;
  onTimeoutText: (text: string) => void;
  onTimeoutReset: () => void;
  onTest: (opener: HTMLElement) => void;
  testid: string;
}

export const PreflightFields: Component<PreflightFieldsProps> = (props) => {
  const errorId = createUniqueId();
  return (
    <details class="settings-preflight" data-ac-testid={props.testid} data-ac-role="group">
      <summary data-ac-testid={`${props.testid}.summary`}>Preflight</summary>
      <label class="settings-field">
        <span class="settings-label">Preflight command</span>
        <input
          class="settings-input settings-preflight-command"
          value={props.command ?? ""}
          placeholder="Optional command or script"
          spellcheck={false}
          onInput={(e) => props.onCommand(e.currentTarget.value)}
          data-ac-testid={`${props.testid}.command`}
          data-ac-role="textbox"
        />
      </label>
      <div class="settings-label-hint">{LEVEL_HELP[props.level]}</div>
      <div class="settings-label-hint">{COMMON_HELP}</div>
      <div class="settings-preflight-timeout">
        <label class="settings-field">
          <span class="settings-label">Timeout (seconds)</span>
          <input
            class="settings-input"
            classList={{ invalid: props.timeoutInvalid }}
            inputMode="numeric"
            value={props.timeoutText}
            aria-invalid={props.timeoutInvalid ? "true" : undefined}
            aria-describedby={props.timeoutInvalid ? errorId : undefined}
            onInput={(e) => props.onTimeoutText(e.currentTarget.value)}
            data-ac-testid={`${props.testid}.timeout`}
            data-ac-role="textbox"
            data-ac-state={props.timeoutInvalid ? "invalid" : "valid"}
          />
        </label>
        <button
          type="button"
          class="settings-profile-cell-btn"
          onClick={() => props.onTimeoutReset()}
          data-ac-testid={`${props.testid}.timeoutReset`}
          data-ac-role="button"
        >
          Reset to default
        </button>
      </div>
      <div class="settings-label-hint">{TIMEOUT_HELP}</div>
      <Show when={props.level === "profile"}>
        <div class="settings-label-hint">{PROFILE_TIMEOUT_HELP}</div>
      </Show>
      <Show when={props.timeoutInvalid}>
        <div
          id={errorId}
          class="settings-profile-cell-error"
          data-ac-testid={`${props.testid}.timeout.error`}
          data-ac-role="status"
        >
          {INVALID_TIMEOUT_TEXT}
        </div>
      </Show>
      <button
        type="button"
        class="settings-profile-cell-btn settings-preflight-test"
        onClick={(e) => props.onTest(e.currentTarget)}
        data-ac-testid={`${props.testid}.test`}
        data-ac-role="button"
      >
        Test preflights
      </button>
    </details>
  );
};
