import type { Component } from "solid-js";

/** #881 — monochrome Heroicons archive-box glyph for archive entry points. */
const ArchiveIcon: Component<{ class?: string }> = (props) => (
  <svg class={props.class} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M3.375 3C2.339 3 1.5 3.84 1.5 4.875v.75C1.5 6.66 2.34 7.5 3.375 7.5h17.25c1.035 0 1.875-.84 1.875-1.875v-.75C22.5 3.839 21.66 3 20.625 3H3.375Z" />
    <path
      fill-rule="evenodd"
      d="m3.087 9 .54 9.176A3 3 0 0 0 6.62 21h10.757a3 3 0 0 0 2.995-2.824L20.913 9H3.087Zm6.163 3.75A.75.75 0 0 1 10 12h4a.75.75 0 0 1 0 1.5h-4a.75.75 0 0 1-.75-.75Z"
      clip-rule="evenodd"
    />
  </svg>
);

export default ArchiveIcon;
