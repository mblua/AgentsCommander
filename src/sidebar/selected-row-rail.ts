/** #1796 - the selected-row rail is a `border-left` on every session and replica
 *  row. Two settings drive it, and the same two predicates decide both the red
 *  hint in Settings > General and whether a value is published to the DOM.
 *  Keeping one source for both is the point of this module: a value that warns
 *  must not reach the stylesheet, and a value that does not warn must. */

export const RAIL_WIDTH_DEFAULT = "9px";
export const RAIL_COLOR_DEFAULT = "#630707";

export const RAIL_WIDTH_PROPERTY = "--ac-selected-rail-width";
export const RAIL_COLOR_PROPERTY = "--ac-selected-rail-color";

/** Accepted widths are the whole numbers 1 to `RAIL_WIDTH_MAX`, optionally
 *  suffixed `px`. The bound is not cosmetic: a wider rail is paid for by
 *  shrinking the row's left padding, and padding cannot go negative, so row
 *  content keeps its column only while the width is no wider than the padding
 *  plus border the winning rule drew. That inset is `max(width, padding +
 *  border)`; its smallest value in sidebar.css is 14px, on a project session row
 *  in deep-space, arctic-ops and neon-circuit. The cap equals it: nothing moves. */
export const RAIL_WIDTH_MAX = 14;

const WIDTH_RE = /^(?:1[0-4]|[1-9])(?:px)?$/;
const COLOR_RE = /^#[0-9a-fA-F]{6}$/;

/** Shape only, never a computed length. A whole number from 1 to 14 with an
 *  optional `px`. `0`, `15` and up, and any leading zero such as `09` are all
 *  rejected. Empty warns, unlike `defaultShell`: an empty rail width has no "use
 *  the default" reading, and a silent fallback to 9px would look like the
 *  setting was ignored. */
export function isValidRailWidth(value: string): boolean {
  return WIDTH_RE.test(value.trim());
}

/** Shape only. `#` plus exactly six hex digits. The 3- and 8-digit forms are
 *  rejected on purpose, so the hint, the writer and the docs describe one set of
 *  values. */
export function isValidRailColor(value: string): boolean {
  return COLOR_RE.test(value.trim());
}

/** `12` and `12px` both persist as the user typed them; the stylesheet needs a
 *  length. Only called on a value that already passed `isValidRailWidth`. */
export function railWidthToCss(value: string): string {
  const trimmed = value.trim();
  return /^\d{1,2}$/.test(trimmed) ? `${trimmed}px` : trimmed;
}

/** Publishes both values as inline custom properties on `root`, which is
 *  `document.documentElement` in both callers. An inline declaration beats the
 *  `:root` rule in variables.css, and `--ac-rail-delta` is computed on this same
 *  element, so the padding compensation follows the published width.
 *
 *  An invalid value REMOVES its property rather than writing it. That is what
 *  makes "a malformed value degrades at the CSS layer" true: the `:root`
 *  declaration then supplies the factory value. Writing the raw text instead
 *  would make `border-left-width` invalid at computed-value time and take the
 *  padding compensation down with it, which is a broken layout, not a
 *  degradation. */
export function applySelectedRowRail(
  root: HTMLElement,
  width: string,
  color: string
): void {
  if (isValidRailWidth(width)) {
    root.style.setProperty(RAIL_WIDTH_PROPERTY, railWidthToCss(width));
  } else {
    root.style.removeProperty(RAIL_WIDTH_PROPERTY);
  }
  if (isValidRailColor(color)) {
    root.style.setProperty(RAIL_COLOR_PROPERTY, color.trim());
  } else {
    root.style.removeProperty(RAIL_COLOR_PROPERTY);
  }
}
