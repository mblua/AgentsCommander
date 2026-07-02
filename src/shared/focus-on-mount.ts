/**
 * #746 — focus (and optionally select) an element right after it mounts.
 *
 * Solid `ref` callbacks run while the element is being created, before it is
 * connected to the document, where `.focus()` is a silent no-op — so the focus
 * must be deferred one frame. Guarded-rAF with a setTimeout fallback, mirroring
 * ProjectPanel's flyout-reclamp idiom. If the element was disposed before the
 * frame fires, the focus call is a harmless no-op.
 */
export function focusOnMount(el: HTMLElement, options: { select?: boolean } = {}): void {
  const run = () => {
    el.focus();
    if (options.select && el instanceof HTMLInputElement) el.select();
  };
  if (typeof window.requestAnimationFrame === "function") {
    window.requestAnimationFrame(run);
  } else {
    window.setTimeout(run, 0);
  }
}
