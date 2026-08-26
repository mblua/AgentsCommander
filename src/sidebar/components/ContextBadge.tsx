import { Component, Show } from "solid-js";
import { contextBadgeText } from "./session-context";

export const CONTEXT_BADGE_TOOLTIP =
  "Context window in use, read from what the agent draws in its terminal. " +
  "Best-effort: it can be unavailable, stale or absent. A high reading does not " +
  "mean this session must be restarted.";

const ContextBadge: Component<{
  percent: number | null | undefined;
  testId?: string;
  testIdPrivate?: boolean;
}> = (props) => {
  const reading = (): { value: number } | undefined => {
    const percent = props.percent;
    return percent === null || percent === undefined ? undefined : { value: percent };
  };

  return (
    <Show
      when={reading()}
      fallback={
        <span
          class="ctx-badge unavailable"
          title={CONTEXT_BADGE_TOOLTIP}
          data-ac-testid={props.testId}
          data-ac-testid-private={props.testIdPrivate ? "true" : undefined}
          data-ac-role="status"
          data-ac-state="unavailable"
        >
          {contextBadgeText(props.percent)}
        </span>
      }
    >
      {(value) => (
        <span
          class="ctx-badge"
          role="meter"
          aria-label="Context window used"
          aria-valuenow={value().value}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuetext={`Context ${value().value}% used`}
          title={CONTEXT_BADGE_TOOLTIP}
          data-ac-testid={props.testId}
          data-ac-testid-private={props.testIdPrivate ? "true" : undefined}
          data-ac-role="status"
          data-ac-state="reading"
        >
          {contextBadgeText(value().value)}
        </span>
      )}
    </Show>
  );
};

export default ContextBadge;
