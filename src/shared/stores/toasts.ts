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
}

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  exiting: boolean;
  action?: ToastAction;
  secondaryAction?: ToastAction;
  tag?: string;
}

// Errors stay until dismissed (the #574 failure MUST be noticed); info/success
// auto-dismiss. Matches the existing loop toast's ~3s feel for non-errors.
const DEFAULT_DURATION_MS: Record<ToastKind, number | null> = {
  error: null,
  success: 4000,
  info: 4000,
};

// Cap on visible toasts. Past this, eviction is KIND-AWARE (#574 §15.3): the
// oldest NON-error (auto-dismissing) toast is evicted first, so a transient
// info/success can never silently bury an unread sticky error; only when every
// visible toast is an error do we fall back to evicting the oldest error.
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
        setToasts(existingIndex, "message", opts.message);
        setToasts(existingIndex, "action", opts.action);
        setToasts(existingIndex, "secondaryAction", opts.secondaryAction);
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
    };

    const evicted: number[] = [];
    const next = [...toasts, toast];
    // Kind-aware eviction (§15.3): evict the oldest NON-error first so an
    // unread sticky error is never silently dropped by a transient toast.
    // Fall back to the oldest overall only when all visible toasts are errors
    // (keeps MAX_VISIBLE an honest hard cap).
    while (next.length > MAX_VISIBLE) {
      let victim = next.findIndex((t) => t.kind !== "error");
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
