// #1871 phase 1 - context-menu MECHANICS only, private to context-menu/.
// Portal, the session-context-menu div, viewport clamping, the window
// click / contextmenu / keydown(Escape) dismissal with its cleanup, and one
// keyed flyout slot with its open/close scheduling and positioning. It knows
// nothing about items: SessionRowMenu owns those. Ported from
// RootAgentBanner's hand-rolled copy and ProjectPanel's replica-menu copy, with
// two deliberate departures marked CHANGE 1 and CHANGE 2 below.
import {
  createEffect,
  createSignal,
  on,
  onCleanup,
  Show,
  untrack,
  type Component,
  type JSX,
} from "solid-js";
import { Portal } from "solid-js/web";

const CONTEXT_MENU_VIEWPORT_MARGIN = 8;
const FLYOUT_CLOSE_GRACE_MS = 180;
const FLYOUT_FALLBACK_WIDTH = 220;
const FLYOUT_FALLBACK_HEIGHT = 88;

export interface MenuPoint {
  x: number;
  y: number;
}

/** The child API handed to the catalogue. Phase 1 has exactly two producers of
 *  a non-null flyout key: a repo entry opens `"repo:" + index` and the Add to
 *  Group trigger opens the literal `"addToGroup"`. */
export interface ContextMenuSurfaceApi {
  openFlyout(key: string, anchor: HTMLElement): void;
  scheduleFlyoutClose(): void;
  cancelFlyoutClose(): void;
  closeFlyout(): void;
  flyoutKey(): string | null;
  flyoutPos(): MenuPoint | null;
  /** The rendered flyout panel, for measuring and for focusing its first item. */
  setFlyoutEl(el: HTMLDivElement | undefined): void;
  focusFirstFlyoutItem(): void;
  /** Re-measure and re-clamp the menu after an inline expansion changed its
   *  height (CHANGE 2). A no-op once the menu has closed. */
  reclamp(): void;
}

export interface ContextMenuSurfaceProps {
  open: boolean;
  x: number;
  y: number;
  testId: string;
  onDismiss: () => void;
  children: (surface: ContextMenuSurfaceApi) => JSX.Element;
}

const deferToFrame = (fn: () => void): void => {
  if (typeof window.requestAnimationFrame === "function") {
    window.requestAnimationFrame(fn);
    return;
  }
  window.setTimeout(fn, 0);
};

const clampAxis = (requested: number, size: number, viewport: number): number => {
  const max = Math.max(
    CONTEXT_MENU_VIEWPORT_MARGIN,
    viewport - size - CONTEXT_MENU_VIEWPORT_MARGIN,
  );
  return Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, requested), max);
};

const ContextMenuSurface: Component<ContextMenuSurfaceProps> = (props) => {
  const [pos, setPos] = createSignal<MenuPoint>({ x: 0, y: 0 });
  const [flyoutKey, setFlyoutKey] = createSignal<string | null>(null);
  const [flyoutPos, setFlyoutPos] = createSignal<MenuPoint | null>(null);
  let menuEl: HTMLDivElement | undefined;
  let flyoutEl: HTMLDivElement | undefined;
  let flyoutAnchorEl: HTMLElement | undefined;
  let registerTimer: number | undefined;
  let flyoutCloseTimer: number | undefined;
  let dismissHandler: ((ev?: Event) => void) | null = null;

  const clampTo = (x: number, y: number): void => {
    if (!menuEl) return;
    const { width, height } = menuEl.getBoundingClientRect();
    setPos({
      x: clampAxis(x, width, window.innerWidth),
      y: clampAxis(y, height, window.innerHeight),
    });
  };

  // CHANGE 1: the registration timer handle is stored and cancelled, so a
  // dismiss or an unmount before it fires cannot leak three window listeners
  // that nothing can ever remove. The flyout half is not decoration either: a
  // close scheduled 180 ms out must not fire against a surface that is gone,
  // and a surviving key must not render a flyout on the next open against an
  // anchor that no longer exists. Idempotent, so a double dismiss is safe.
  const teardown = (): void => {
    if (registerTimer !== undefined) {
      window.clearTimeout(registerTimer);
      registerTimer = undefined;
    }
    if (flyoutCloseTimer !== undefined) {
      window.clearTimeout(flyoutCloseTimer);
      flyoutCloseTimer = undefined;
    }
    if (dismissHandler) {
      window.removeEventListener("click", dismissHandler);
      window.removeEventListener("contextmenu", dismissHandler);
      window.removeEventListener("keydown", dismissHandler as EventListener);
    }
    dismissHandler = null;
    setFlyoutKey(null);
    setFlyoutPos(null);
    flyoutAnchorEl = undefined;
    flyoutEl = undefined;
  };

  createEffect(
    on(
      () => [props.open, props.x, props.y] as const,
      ([open, x, y]) => {
        teardown();
        if (!open) return;
        setPos({ x, y });
        const dismiss = (ev?: Event): void => {
          if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
          teardown();
          props.onDismiss();
        };
        registerTimer = window.setTimeout(() => {
          registerTimer = undefined;
          // Covers a handle cleared after the callback was already taken off
          // the timer queue.
          if (!props.open) return;
          clampTo(x, y);
          dismissHandler = dismiss;
          window.addEventListener("click", dismiss);
          window.addEventListener("contextmenu", dismiss);
          window.addEventListener("keydown", dismiss as EventListener);
        }, 0);
      },
    ),
  );
  onCleanup(teardown);

  // CHANGE 2: the catalogue drives this from a createEffect over the two
  // inline expansions, so no host has to remember to reclamp.
  const reclamp = (): void => {
    if (!props.open) return;
    deferToFrame(() => {
      if (!props.open) return;
      const current = pos();
      clampTo(current.x, current.y);
    });
  };

  const cancelFlyoutClose = (): void => {
    if (flyoutCloseTimer === undefined) return;
    window.clearTimeout(flyoutCloseTimer);
    flyoutCloseTimer = undefined;
  };

  // Clears the slot unconditionally, with no key comparison, exactly as
  // closeRepoFlyout does; correctness comes from teardown() and openFlyout()
  // both cancelling the pending close.
  const closeFlyout = (): void => {
    cancelFlyoutClose();
    setFlyoutKey(null);
    setFlyoutPos(null);
    flyoutAnchorEl = undefined;
    flyoutEl = undefined;
  };

  const scheduleFlyoutClose = (): void => {
    cancelFlyoutClose();
    flyoutCloseTimer = window.setTimeout(() => {
      flyoutCloseTimer = undefined;
      closeFlyout();
    }, FLYOUT_CLOSE_GRACE_MS);
  };

  // positionRepoFlyout's algorithm verbatim: right of the anchor, flipped to
  // the left when it would overflow, clamped inside the margin on both axes,
  // with the 220 x 88 fallback when the panel is not laid out yet.
  const positionFlyout = (anchor: HTMLElement): void => {
    const rect = anchor.getBoundingClientRect();
    const width = flyoutEl?.getBoundingClientRect().width ?? FLYOUT_FALLBACK_WIDTH;
    const height = flyoutEl?.getBoundingClientRect().height ?? FLYOUT_FALLBACK_HEIGHT;
    let x = rect.right + 4;
    if (x + width + CONTEXT_MENU_VIEWPORT_MARGIN > window.innerWidth) {
      x = rect.left - width - 4;
    }
    setFlyoutPos({
      x: clampAxis(x, width, window.innerWidth),
      y: clampAxis(rect.top, height, window.innerHeight),
    });
  };

  const reclampFlyout = (): void => {
    const anchor = flyoutAnchorEl;
    if (!anchor?.isConnected || flyoutKey() === null) return;
    deferToFrame(() => {
      if (anchor !== flyoutAnchorEl || !anchor.isConnected || flyoutKey() === null) return;
      positionFlyout(anchor);
    });
  };

  // cancelFlyoutClose() is the first statement on purpose: with a single slot,
  // a close scheduled while flyout A was showing would otherwise fire 180 ms
  // after flyout B opened and close B.
  const openFlyout = (key: string, anchor: HTMLElement): void => {
    cancelFlyoutClose();
    if (flyoutKey() !== key) flyoutEl = undefined;
    flyoutAnchorEl = anchor;
    positionFlyout(anchor);
    setFlyoutKey(key);
    reclampFlyout();
  };

  const focusFirstFlyoutItem = (): void => {
    queueMicrotask(() => flyoutEl?.querySelector("button")?.focus());
  };

  const api: ContextMenuSurfaceApi = {
    openFlyout,
    scheduleFlyoutClose,
    cancelFlyoutClose,
    closeFlyout,
    flyoutKey,
    flyoutPos,
    setFlyoutEl: (el) => {
      flyoutEl = el;
    },
    focusFirstFlyoutItem,
    reclamp,
  };

  return (
    <Show when={props.open}>
      <Portal>
        <div
          class="session-context-menu"
          ref={menuEl}
          style={{ left: `${pos().x}px`, top: `${pos().y}px` }}
          onClick={(e) => e.stopPropagation()}
          data-ac-testid={props.testId}
          data-ac-role="menu"
        >
          {untrack(() => props.children(api))}
        </div>
      </Portal>
    </Show>
  );
};

export default ContextMenuSurface;
