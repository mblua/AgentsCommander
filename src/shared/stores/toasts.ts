import { createSignal } from "solid-js";

export type ToastKind = "error" | "success" | "info";

export interface PushToastOptions {
  message: string;
  kind?: ToastKind;            // default "info"
  /** Auto-dismiss delay. `null` = sticky (stays until dismissed). Omit to use
   *  the per-kind default below. */
  durationMs?: number | null;
}

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
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

const [toasts, setToasts] = createSignal<Toast[]>([]);
let nextId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();

function clearTimer(id: number): void {
  const t = timers.get(id);
  if (t !== undefined) {
    clearTimeout(t);
    timers.delete(id);
  }
}

export const toastStore = {
  /** Reactive accessor (read inside JSX / effects to subscribe). */
  get items(): Toast[] {
    return toasts();
  },

  push(opts: PushToastOptions): number {
    const id = nextId++;
    const kind = opts.kind ?? "info";
    const toast: Toast = { id, kind, message: opts.message };

    const evicted: number[] = [];
    setToasts((prev) => {
      const next = [...prev, toast];
      // Kind-aware eviction (§15.3): evict the oldest NON-error first so an
      // unread sticky error is never silently dropped by a transient toast.
      // Fall back to the oldest overall only when all visible toasts are errors
      // (keeps MAX_VISIBLE an honest hard cap). Pure: only mutates the local
      // `next` copy; timer cleanup happens after via `evicted`.
      while (next.length > MAX_VISIBLE) {
        let victim = next.findIndex((t) => t.kind !== "error");
        if (victim === -1) victim = 0;
        evicted.push(next.splice(victim, 1)[0].id);
      }
      return next;
    });
    evicted.forEach(clearTimer);

    const duration =
      opts.durationMs === undefined ? DEFAULT_DURATION_MS[kind] : opts.durationMs;
    if (duration !== null) {
      timers.set(id, setTimeout(() => toastStore.dismiss(id), duration));
    }
    return id;
  },

  dismiss(id: number): void {
    clearTimer(id);
    setToasts((prev) => prev.filter((t) => t.id !== id));
  },

  /** Remove all toasts + cancel all timers (onCleanup + test reset). */
  clear(): void {
    timers.forEach((t) => clearTimeout(t));
    timers.clear();
    setToasts([]);
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
