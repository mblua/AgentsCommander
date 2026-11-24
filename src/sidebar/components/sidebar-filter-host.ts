// Moves the one sidebar filter row into the stuck group header.
import { onCleanup, onMount } from "solid-js";

export interface PanelBox {
  top: number;
  bottom: number;
}

interface HostState {
  row: HTMLElement;
  slot: HTMLElement | null;
  input: HTMLInputElement | null;
  wantFocus: boolean;
  onFocusIn: () => void;
  onFocusOut: () => void;
}

const ROW = '[data-ac-testid="sidebar.regexFilter.row"]';
const SLOT = '[data-ac-testid="sidebar.regexFilter.errorSlot"]';
const hosts = new WeakMap<HTMLElement, HostState>();

/** First panel spanning `edge + 1`; 0 when none does. */
export function pickHostIndex(panels: readonly PanelBox[], edge: number): number {
  const at = edge + 1;
  const i = panels.findIndex((p) => p.top <= at && p.bottom > at);
  return i === -1 ? 0 : i;
}

function detach(state: HostState): void {
  state.input?.removeEventListener("focusin", state.onFocusIn);
  state.input?.removeEventListener("focusout", state.onFocusOut);
}

/** Caches the live row; a detached row (its panel was re-created) stays the control. */
function hostState(scroller: HTMLElement): HostState | undefined {
  const row = scroller.querySelector<HTMLElement>(ROW);
  const cached = hosts.get(scroller);
  if (!row || row === cached?.row) return cached;
  if (cached) detach(cached);
  const input = row.querySelector("input");
  const state: HostState = {
    row,
    slot: scroller.querySelector<HTMLElement>(SLOT),
    input,
    wantFocus: false,
    onFocusIn: () => {
      state.wantFocus = true;
    },
    onFocusOut: () => {
      queueMicrotask(() => {
        if (input?.isConnected && document.activeElement !== input) state.wantFocus = false;
      });
    },
  };
  input?.addEventListener("focusin", state.onFocusIn);
  input?.addEventListener("focusout", state.onFocusOut);
  hosts.set(scroller, state);
  return state;
}

function collectPanelHeaders(scroller: HTMLElement): { headers: HTMLElement[]; boxes: PanelBox[] } {
  const headers: HTMLElement[] = [];
  const boxes: PanelBox[] = [];
  for (const panel of Array.from(scroller.children)) {
    if (!panel.classList.contains("project-panel")) continue;
    const header = panel.querySelector<HTMLElement>(":scope > .project-header");
    if (!header) continue;
    const { top, bottom } = panel.getBoundingClientRect();
    headers.push(header);
    boxes.push({ top, bottom });
  }
  return { headers, boxes };
}

export function placeFilterRow(scroller: HTMLElement): void {
  const { headers, boxes } = collectPanelHeaders(scroller);
  if (headers.length === 0) return;
  const state = hostState(scroller);
  if (!state) return;
  const { row, slot, input } = state;
  const target = headers[pickHostIndex(boxes, scroller.getBoundingClientRect().top)];
  if (!target) return;
  if (row.parentElement === target && (!slot || slot.parentElement === target)) return;

  const hadFocus = !!input && (state.wantFocus || document.activeElement === input);
  const start = input?.selectionStart;
  const end = input?.selectionEnd;
  if (slot) target.append(row, slot);
  else target.appendChild(row);
  if (hadFocus && input) {
    input.focus({ preventScroll: true });
    if (typeof start === "number" && typeof end === "number") input.setSelectionRange(start, end);
  }
}

export function createSidebarFilterHost(scroller: () => HTMLElement | null | undefined): void {
  onMount(() => {
    const el = scroller();
    if (!el) return;
    const run = () => placeFilterRow(el);
    run();
    el.addEventListener("scroll", run, { passive: true });
    window.addEventListener("resize", run);
    const observer = new MutationObserver(run);
    observer.observe(el, { childList: true, subtree: true });
    onCleanup(() => {
      el.removeEventListener("scroll", run);
      window.removeEventListener("resize", run);
      observer.disconnect();
      const state = hosts.get(el);
      if (state) detach(state);
      hosts.delete(el);
    });
  });
}
