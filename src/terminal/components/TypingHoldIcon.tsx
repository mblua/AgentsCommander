import type { Component } from "solid-js";

/**
 * #2379 - monochrome padlock for the typing-hold button. A color-emoji glyph
 * (U+1F512 / U+1F513) ignores `color`, so the open state could not be dimmed
 * and the closed state could not take the status-bar accent. Drawn as SVG with
 * `currentColor` (same approach as RaiseHandIcon, #775), so the button's color
 * paints both states on every platform. `closed` keeps the glyph semantics:
 * open lock = inactive, closed lock = active.
 */
const TypingHoldIcon: Component<{ closed: boolean; class?: string }> = (props) => (
  <svg
    class={props.class}
    viewBox="0 0 24 24"
    fill="currentColor"
    aria-hidden="true"
    data-ac-testid="statusBar.typingHoldIcon"
    data-state={props.closed ? "closed" : "open"}
  >
    <path
      d={props.closed ? "M8 10V7a4 4 0 0 1 8 0V10" : "M8 10V7a4 4 0 0 1 8 0"}
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
    />
    <rect x="4" y="10" width="16" height="11" rx="2" />
  </svg>
);

export default TypingHoldIcon;
