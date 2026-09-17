/**
 * Keeps Tab / Shift+Tab focus inside a modal's focusable set: wraps from the
 * last element to the first (or first to last) and calls preventDefault only
 * when it moves focus itself. Callers own the `e.key === "Tab"` check because
 * each modal has its own propagation rules.
 */
export function trapTabFocus(e: KeyboardEvent, focusables: readonly HTMLElement[]): void {
  if (focusables.length < 2) return;
  const idx = focusables.indexOf(document.activeElement as HTMLElement);
  if (idx === -1) {
    e.preventDefault();
    (e.shiftKey ? focusables[focusables.length - 1] : focusables[0]).focus();
    return;
  }
  if (!e.shiftKey) {
    if (idx === focusables.length - 1) {
      e.preventDefault();
      focusables[0].focus();
    }
    return;
  }
  if (idx <= 0) {
    e.preventDefault();
    focusables[focusables.length - 1].focus();
  }
}
