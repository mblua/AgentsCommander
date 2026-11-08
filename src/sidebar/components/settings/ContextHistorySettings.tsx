// #4049 - Settings > Context Heat Map (desktop only). Renders the three preferences;
// the modal owns the draft, validation and Save/Cancel. No persistence or history here.
import { type Component, Show } from "solid-js";
import type { ContextHistoryColorDays } from "../../../shared/types";

export const CONTEXT_HISTORY_RETENTION_DAYS_MIN = 1;
export const CONTEXT_HISTORY_RETENTION_DAYS_MAX = 365;
export const CONTEXT_HISTORY_RETENTION_DAYS_DEFAULT = 30;
// #3532 Allowed windows for the sidebar context-average color.
const COLOR_DAYS: readonly ContextHistoryColorDays[] = [1, 7, 30];

/** Whole days in range, or null. A blank or a fraction is invalid and never coerced. */
export function parseContextHistoryRetentionDays(raw: string): number | null {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= CONTEXT_HISTORY_RETENTION_DAYS_MIN && value <= CONTEXT_HISTORY_RETENTION_DAYS_MAX
    ? value
    : null;
}

export interface ContextHistorySettingsProps {
  enabled: boolean;
  retentionText: string;
  retentionError: string | null;
  colorDays: ContextHistoryColorDays;
  onEnabled: (enabled: boolean) => void;
  onRetentionText: (raw: string) => void;
  onColorDays: (days: ContextHistoryColorDays) => void;
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
      <span class="settings-label">Período para el color de la media de contexto</span>
      <select
        class="settings-input"
        value={String(props.colorDays)}
        onChange={(e) => {
          const raw = e.currentTarget.value;
          const days = COLOR_DAYS.find((d) => String(d) === raw);
          if (days !== undefined) props.onColorDays(days);
        }}
        data-ac-testid="settings.contextHistory.colorDays"
      >
        <option value="1">Último día</option>
        <option value="7">Últimos 7 días</option>
        <option value="30">Últimos 30 días (predeterminado)</option>
      </select>
      <div class="settings-hint" data-ac-testid="settings.contextHistory.colorDays.hint">
        Colorea la media de contexto del agente o del team, no el CTX vivo; el hover
        muestra siempre las medias de 1, 7 y 30 días. No cambia la captura ni el
        período del historial.
      </div>
    </label>
  </div>
);

export default ContextHistorySettings;
