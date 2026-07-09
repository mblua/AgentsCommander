import type { Component } from "solid-js";

/**
 * #895 — the "clear this selection" glyph. Distinct from [[TrashIcon]]: this one
 * drops a choice, it never deletes anything, so it is never tinted red. Callers
 * size/tint it via the passed-in `class` (fill: currentColor). Heroicons `x-mark`.
 */
const XMarkIcon: Component<{ class?: string }> = (props) => (
  <svg class={props.class} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path
      fill-rule="evenodd"
      clip-rule="evenodd"
      d="M5.47 5.47a.75.75 0 0 1 1.06 0L12 10.94l5.47-5.47a.75.75 0 1 1 1.06 1.06L13.06 12l5.47 5.47a.75.75 0 1 1-1.06 1.06L12 13.06l-5.47 5.47a.75.75 0 0 1-1.06-1.06L10.94 12 5.47 6.53a.75.75 0 0 1 0-1.06Z"
    />
  </svg>
);

export default XMarkIcon;
