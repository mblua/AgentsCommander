import { Component, Show } from "solid-js";
import { contextBadgeText } from "./session-context";

/**
 * #1033 - the honesty requirement, and the only place it is user-visible.
 * Carried by BOTH states, because a reading and its absence are equally best-effort.
 */
export const CONTEXT_BADGE_TOOLTIP =
  "Context window in use, read from what the agent draws in its terminal. " +
  "Best-effort: it can be unavailable, stale or absent. A high reading does not " +
  "mean this session must be restarted.";

/**
 * #1033 - the CTX badge: how much of its context window an agent session has used,
 * scraped from what the agent draws in its own terminal.
 *
 * Shared by SessionItem, the ProjectPanel replica rows, and RootAgentBanner so the
 * badge looks and reads identically everywhere a coding-agent session can show one;
 * triplicating the ARIA markup across those three files is how the three drift. The
 * caller supplies the testid because each surface names itself.
 *
 * THE BADGE IS A SIGNAL, NEVER A CONTROL. A `<span>`, never a `<button>` - unlike
 * ProfileOutdatedBadge, which this otherwise copies. No onClick, no onKeyDown, no
 * tabindex, no cursor change, no threshold, and nothing anywhere reads this value.
 * AC has no denominator for the number (one rounded integer, unknown window size),
 * so it has no basis for a threshold and invents none.
 *
 * The two states are structurally DIFFERENT elements rather than one element with
 * nullable attributes: `role="meter"` requires `aria-valuenow`, and there is no
 * valid `aria-valuenow` for N/A, so a meter with no value would be invalid ARIA.
 *
 * No aria-live on either state, deliberately: the reading refreshes on a 5s cadence,
 * so a live region would interrupt a screen reader every 5 seconds, forever, with a
 * number that drives nothing. That is an accessibility defect, not a feature.
 */
const ContextBadge: Component<{
  percent: number | null | undefined;
  testId?: string;
}> = (props) => {
  // Wrapped in an object on purpose: `<Show when={props.percent}>` would treat a
  // REAL reading of 0% as absent and render N/A, since 0 is falsy. The wrapper is
  // always truthy when there is a reading, and it narrows `number | null |
  // undefined` to `number` without a cast.
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
