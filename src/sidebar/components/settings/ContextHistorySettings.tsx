// #4049 - Settings > Context Heat Map (desktop only). Renders the preferences;
// the modal owns the draft, validation and Save/Cancel. No persistence or history here.
import { type Component, For, Show } from "solid-js";
import { sessionWindowLabel } from "../../../shared/context-history";

export const CONTEXT_HISTORY_RETENTION_DAYS_MIN = 1;
export const CONTEXT_HISTORY_RETENTION_DAYS_MAX = 365;
export const CONTEXT_HISTORY_RETENTION_DAYS_DEFAULT = 30;
// #4091 Session-window list bounds; the backend owns the default list.
export const CONTEXT_HISTORY_SESSION_WINDOWS_MAX_LEN = 8;
export const CONTEXT_HISTORY_SESSION_WINDOW_MAX = 10000;

/** Whole days in range, or null. A blank or a fraction is invalid and never coerced. */
export function parseContextHistoryRetentionDays(raw: string): number | null {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= CONTEXT_HISTORY_RETENTION_DAYS_MIN && value <= CONTEXT_HISTORY_RETENTION_DAYS_MAX
    ? value
    : null;
}

/**
 * #4091 Comma-separated session counts: 1..8 ascending distinct whole numbers
 * in 1..10000, or null. Never sorts, deduplicates or drops a blank token.
 */
export function parseContextHistorySessionWindows(raw: string): number[] | null {
  const tokens = raw.split(",").map((token) => token.trim());
  if (tokens.length > CONTEXT_HISTORY_SESSION_WINDOWS_MAX_LEN) return null;
  const counts: number[] = [];
  for (const token of tokens) {
    if (!/^\d+$/.test(token)) return null;
    const value = Number(token);
    if (value < 1 || value > CONTEXT_HISTORY_SESSION_WINDOW_MAX) return null;
    const previous = counts[counts.length - 1];
    if (previous !== undefined && value <= previous) return null;
    counts.push(value);
  }
  return counts;
}

export interface ContextHistorySettingsProps {
  enabled: boolean;
  retentionText: string;
  retentionError: string | null;
  windowsText: string;
  windowsError: string | null;
  windows: readonly number[];
  colorSessions: number | null;
  onEnabled: (enabled: boolean) => void;
  onRetentionText: (raw: string) => void;
  onWindowsText: (raw: string) => void;
  onColorSessions: (sessions: number) => void;
}

const ContextHistorySettings: Component<ContextHistorySettingsProps> = (props) => (
  <div class="settings-section">
    <div class="settings-section-title">Context Heat Map</div>
    <label class="settings-checkbox-field">
      <input
        type="checkbox"
        class="settings-checkbox"
        checked={props.enabled}
        onChange={(e) => props.onEnabled(e.currentTarget.checked)}
        data-ac-testid="settings.contextHistory.enabled"
      />
      <span>Record context history</span>
    </label>
    <label class="settings-field">
      <span class="settings-label">Keep history (days)</span>
      <input
        class="settings-input settings-input-sm"
        type="number"
        min={CONTEXT_HISTORY_RETENTION_DAYS_MIN}
        max={CONTEXT_HISTORY_RETENTION_DAYS_MAX}
        step="1"
        value={props.retentionText}
        onInput={(e) => props.onRetentionText(e.currentTarget.value)}
        data-ac-testid="settings.contextHistory.retentionDays"
        data-ac-role="spinbutton"
      />
    </label>
    <Show when={props.retentionError}>
      {(message) => (
        <div
          class="settings-hint settings-hint-error"
          data-ac-testid="settings.contextHistory.retentionDays.error"
        >
          {message()}
        </div>
      )}
    </Show>
    <label class="settings-field">
      <span class="settings-label">Session windows</span>
      <input
        class="settings-input"
        type="text"
        value={props.windowsText}
        onInput={(e) => props.onWindowsText(e.currentTarget.value)}
        data-ac-testid="settings.contextHistory.sessionWindows"
      />
      <div class="settings-hint" data-ac-testid="settings.contextHistory.sessionWindows.hint">
        Comma-separated session counts: 1 to 8 ascending, distinct whole numbers from 1 to
        10000. The heat map groups each agent's history by its latest sessions.
      </div>
    </label>
    <Show when={props.windowsError}>
      {(message) => (
        <div
          class="settings-hint settings-hint-error"
          data-ac-testid="settings.contextHistory.sessionWindows.error"
        >
          {message()}
        </div>
      )}
    </Show>
    <label class="settings-field">
      <span class="settings-label">Período para el color de la media de contexto</span>
      <select
        class="settings-input"
        value={props.colorSessions === null ? "" : String(props.colorSessions)}
        onChange={(e) => {
          const raw = e.currentTarget.value;
          const sessions = props.windows.find((count) => String(count) === raw);
          if (sessions !== undefined) props.onColorSessions(sessions);
        }}
        data-ac-testid="settings.contextHistory.colorSessions"
      >
        <For each={props.windows}>
          {(count) => <option value={String(count)}>{sessionWindowLabel(count)}</option>}
        </For>
      </select>
      <div class="settings-hint" data-ac-testid="settings.contextHistory.colorSessions.hint">
        Colorea la media de contexto del agente o del team en esa ventana de sesiones, no
        el CTX vivo; el hover muestra todas las ventanas de sesiones. No cambia la captura
        ni el período del historial.
      </div>
    </label>
  </div>
);

export default ContextHistorySettings;
