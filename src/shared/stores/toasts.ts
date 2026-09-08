import { createStore, reconcile } from "solid-js/store";

export type ToastKind = "error" | "success" | "info";

export interface ToastAction {
  label: string;
  onClick: () => void;
  /** #1669: when false, clicking the action runs `onClick` and LEAVES the
   *  toast up. Omitted or true keeps the original behavior (dismiss on click). */
  dismissOnClick?: boolean;
}

export interface PushToastOptions {
  message: string;
  kind?: ToastKind;            // default "info"
  /** Auto-dismiss delay. `null` = sticky (stays until dismissed). Omit to use
   *  the per-kind default below. */
  durationMs?: number | null;
  action?: ToastAction;
  /** #1669: optional second action rendered BEFORE `action`. */
  secondaryAction?: ToastAction;
  tag?: string;
  /** #1857: exempt this toast from kind-aware eviction. Exactly one such toast
   *  exists, the aggregated blocked-menu notice, so `MAX_VISIBLE` remains an
   *  honest cap: the last eviction tier can still take it. */
  pinned?: boolean;
}

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  exiting: boolean;
  action?: ToastAction;
  secondaryAction?: ToastAction;
  tag?: string;
  pinned?: boolean;
}

// Errors stay until dismissed (the #574 failure MUST be noticed); info/success
// auto-dismiss. Matches the existing loop toast's ~3s feel for non-errors.
const DEFAULT_DURATION_MS: Record<ToastKind, number | null> = {
  error: null,
  success: 4000,
  info: 4000,
};

// Cap on visible toasts. Past this, eviction walks three ordered tiers (#574
// §15.3, #1857): the oldest NON-error unpinned toast, then the oldest unpinned
// toast of any kind, then the oldest toast overall. A `pinned` toast is exempt
// from the first two tiers only, so the aggregated blocked-menu notice outlives
// a full stack of sticky errors while MAX_VISIBLE stays an honest hard cap.
const MAX_VISIBLE = 4;
export const TOAST_EXIT_MS = 180;

const [toasts, setToasts] = createStore<Toast[]>([]);
let nextId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();
const exitTimers = new Map<number, ReturnType<typeof setTimeout>>();

function clearTimer(id: number): void {
  const t = timers.get(id);
  if (t !== undefined) {
    clearTimeout(t);
    timers.delete(id);
  }
}

function clearExitTimer(id: number): void {
  const t = exitTimers.get(id);
  if (t !== undefined) {
    clearTimeout(t);
    exitTimers.delete(id);
  }
}

function clearToastTimers(id: number): void {
  clearTimer(id);
  clearExitTimer(id);
}

function removeToastImmediately(id: number): void {
  clearToastTimers(id);
  setToasts(reconcile(toasts.filter((t) => t.id !== id), { key: "id" }));
}

function startToastExit(id: number): void {
  clearTimer(id);

  const index = toasts.findIndex((toast) => toast.id === id);
  if (index === -1 || toasts[index].exiting || exitTimers.has(id)) return;

  setToasts(index, "exiting", true);
  exitTimers.set(
    id,
    setTimeout(() => {
      removeToastImmediately(id);
    }, TOAST_EXIT_MS),
  );
}

export const toastStore = {
  /** Reactive accessor (read inside JSX / effects to subscribe). */
  get items(): Toast[] {
    return toasts;
  },

  push(opts: PushToastOptions): number {
    if (opts.tag) {
      const existingIndex = toasts.findIndex((toast) => toast.tag === opts.tag);
      if (existingIndex !== -1) {
        // #1857: a tagged re-push onto a DYING toast must revive it. Without
        // this, the exit timer armed by the dismiss deletes the freshly painted
        // message 180 ms after that dismiss, counted from the dismiss.
        const existing = toasts[existingIndex];
        if (existing.exiting) {
          clearExitTimer(existing.id);
          setToasts(existingIndex, "exiting", false);
          // `startToastExit` cleared the duration timer and this branch returns
          // without re-arming it, so a revived non-sticky toast would otherwise
          // stay on screen forever. Re-arm ONLY on this revive path: doing it on
          // every tag update would stop a repeatedly re-pushed info toast from
          // ever auto-dismissing, a behaviour change for existing callers. This
          // branch never patches `kind`, so the existing kind is the right
          // default source. This is a no-op for a `durationMs: null` toast.
          const revivedDuration =
            opts.durationMs === undefined
              ? DEFAULT_DURATION_MS[existing.kind]
              : opts.durationMs;
          if (revivedDuration !== null) {
            timers.set(
              existing.id,
              setTimeout(() => toastStore.dismiss(existing.id), revivedDuration),
            );
          }
        }
        setToasts(existingIndex, "message", opts.message);
        setToasts(existingIndex, "action", opts.action);
        setToasts(existingIndex, "secondaryAction", opts.secondaryAction);
        setToasts(existingIndex, "pinned", opts.pinned);
        return toasts[existingIndex].id;
      }
    }

    const id = nextId++;
    const kind = opts.kind ?? "info";
    const toast: Toast = {
      id,
      kind,
      message: opts.message,
      exiting: false,
      action: opts.action,
      secondaryAction: opts.secondaryAction,
      tag: opts.tag,
      pinned: opts.pinned,
    };

    const evicted: number[] = [];
    const next = [...toasts, toast];
    // Three ordered eviction tiers (§15.3, #1857). Tier 1 keeps the kind-aware
    // rule for unpinned toasts: a transient info/success goes before an unread
    // sticky error. Tier 2 takes an unpinned ERROR, which fires when a pinned
    // toast arrives and every other visible toast is an error. That trade-off is
    // deliberate: a blocked session is unanswerable work the user asked to be
    // un-losable, an error is a report. Tier 3 takes index 0 unconditionally; it
    // is unreachable while only one toast is ever pinned, and exists so the loop
    // always terminates and the cap cannot quietly become 5.
    while (next.length > MAX_VISIBLE) {
      let victim = next.findIndex((t) => t.kind !== "error" && !t.pinned);
      if (victim === -1) victim = next.findIndex((t) => !t.pinned);
      if (victim === -1) victim = 0;
      evicted.push(next.splice(victim, 1)[0].id);
    }
    setToasts(reconcile(next, { key: "id" }));
    evicted.forEach(clearToastTimers);

    const duration =
      opts.durationMs === undefined ? DEFAULT_DURATION_MS[kind] : opts.durationMs;
    if (duration !== null && toasts.some((t) => t.id === id)) {
      timers.set(id, setTimeout(() => toastStore.dismiss(id), duration));
    }
    return id;
  },

  dismiss(id: number): void {
    startToastExit(id);
  },

  dismissByTag(tag: string): void {
    for (const toast of toasts) {
      if (toast.tag === tag) startToastExit(toast.id);
    }
  },

  /** Remove all toasts + cancel all timers (onCleanup + test reset). */
  clear(): void {
    timers.forEach((t) => clearTimeout(t));
    timers.clear();
    exitTimers.forEach((t) => clearTimeout(t));
    exitTimers.clear();
    setToasts(reconcile([]));
  },

  // Convenience wrappers.
  error(message: string, opts?: Omit<PushToastOptions, "message" | "kind">): number {
    return toastStore.push({ ...opts, message, kind: "error" });
  },
  info(message: string, opts?: Omit<PushToastOptions, "message" | "kind">): number {
    return toastStore.push({ ...opts, message, kind: "info" });
  },
  success(message: string, opts?: Omit<PushToastOptions, "message" | "kind">): number {
    return toastStore.push({ ...opts, message, kind: "success" });
  },
};
