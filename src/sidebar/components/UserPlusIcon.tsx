import type { Component } from "solid-js";

/**
 * #1731 - the "add someone to this group" glyph for the Add to Group row in the
 * replica context menu. It takes the slot the 👥 emoji used to hold; that emoji
 * moved to Create new group, which reads as "a group" and is what that row
 * produces. Callers size/tint it via the passed-in `class`
 * (stroke: currentColor), exactly like [[DetachIcon]].
 *
 * Deliberate deviation from house style: [[DetachIcon]] and [[ReattachIcon]] are
 * SOLID Heroicons (fill="currentColor"); this is the STROKED Lucide `user-plus`.
 * That is the version reviewed and approved at the menu's real 14px size. Do not
 * silently substitute a solid variant.
 */
const UserPlusIcon: Component<{ class?: string }> = (props) => (
  <svg
    class={props.class}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
    <circle cx="9" cy="7" r="4" />
    <line x1="19" y1="8" x2="19" y2="14" />
    <line x1="22" y1="11" x2="16" y2="11" />
  </svg>
);

export default UserPlusIcon;
