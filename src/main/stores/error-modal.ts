import { batch, createSignal } from "solid-js";
import type { ErrorLogEntry } from "../../shared/types";

// Queue of undismissed ERROR entries. The modal renders entries[index].
// An empty queue means the modal is closed. The queue resets to empty once
// the user dismisses the last entry, so each error burst gets a fresh "1 of N".
const [entries, setEntries] = createSignal<ErrorLogEntry[]>([]);
const [index, setIndex] = createSignal(0);

// Upper bound on the frontend queue (M2). drain() is read-and-clear, so the
// frontend ACCUMULATES entries across successive drains - the Rust-side
// ERROR_BUFFER_CAP does NOT bound this side. Without a cap, a slow error loop
// with the user AFK would grow `entries` without limit (and `enqueue`'s
// array copy would be O(n²) over the run). Mirrors ERROR_BUFFER_CAP.
const FRONTEND_QUEUE_CAP = 200;

export const errorModalStore = {
  get entries(): ErrorLogEntry[] {
    return entries();
  },
  get index(): number {
    return index();
  },
  get total(): number {
    return entries().length;
  },
  /** The entry currently shown, or null when the modal is closed. */
  get current(): ErrorLogEntry | null {
    const e = entries();
    const i = index();
    return e.length > 0 && i < e.length ? e[i] : null;
  },
  /** True while the modal should be visible. */
  get open(): boolean {
    const e = entries();
    return e.length > 0 && index() < e.length;
  },

  /**
   * Append freshly-drained entries. If the modal was closed this re-opens it
   * at the first new entry. If already open, entries are appended after the
   * current one and the "N" in "1 of N" grows live.
   *
   * The queue is capped at FRONTEND_QUEUE_CAP (M2): on overflow the oldest
   * entries are dropped and `index` is shifted down by the drop count so
   * `current` keeps pointing at the same entry the user is viewing (clamped
   * to 0 if that entry was itself evicted under an extreme storm). The two
   * signal writes are `batch`ed so no effect observes a trimmed-entries /
   * stale-index intermediate state.
   */
  enqueue(incoming: ErrorLogEntry[]): void {
    if (incoming.length === 0) return;
    const combined = [...entries(), ...incoming];
    if (combined.length <= FRONTEND_QUEUE_CAP) {
      setEntries(combined);
      return;
    }
    const dropped = combined.length - FRONTEND_QUEUE_CAP;
    batch(() => {
      setEntries(combined.slice(dropped));
      setIndex((i) => Math.max(0, i - dropped));
    });
  },

  /**
   * Dismiss the current entry and advance. When the last entry is dismissed
   * the queue resets, closing the modal.
   */
  dismissCurrent(): void {
    const next = index() + 1;
    if (next >= entries().length) {
      setEntries([]);
      setIndex(0);
    } else {
      setIndex(next);
    }
  },
};

// Test-only reset (gated to Vite MODE === "test"). Mirrors home.ts.
export function __resetErrorModalStoreForTests(): void {
  if (import.meta.env.MODE !== "test") return;
  setEntries([]);
  setIndex(0);
}
