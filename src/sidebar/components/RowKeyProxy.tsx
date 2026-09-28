import type { Component, JSX } from "solid-js";

/** #2655 - keyboard entry for a clickable sidebar row. Native button, stretched
 *  over the row, pointer-events:none: mouse stays on the row, Tab/Enter/Space
 *  land here. Mirrors RootAgentBanner's .root-agent-banner-open. */
export const ROW_KEY_PROXY_CLASS = "ac-row-key-proxy";

export const RowKeyProxy: Component<{ label: string }> = (props) => (
  <button
    type="button"
    class={ROW_KEY_PROXY_CLASS}
    aria-label={props.label}
    // Swallow the browser's key-synthesized click; onRowKey owns activation.
    onClick={(e) => e.stopPropagation()}
  />
);

/** Row onKeyDown: Enter/Space on the row's own direct-child proxy run `action`. */
export function onRowKey(action: () => void): JSX.EventHandler<HTMLElement, KeyboardEvent> {
  return (e) => {
    const t = e.target as Element;
    if (t.parentElement !== e.currentTarget || !t.classList.contains(ROW_KEY_PROXY_CLASS)) return;
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    if (e.repeat) return; // holding the key must not re-fire
    action();
  };
}
