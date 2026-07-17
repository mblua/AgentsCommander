import { batch, createSignal } from "solid-js";
import type { ErrorLogEntry } from "../../shared/types";

const [entries, setEntries] = createSignal<ErrorLogEntry[]>([]);
const [index, setIndex] = createSignal(0);

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
  get current(): ErrorLogEntry | null {
    const e = entries();
    const i = index();
    return e.length > 0 && i < e.length ? e[i] : null;
  },
  get open(): boolean {
    const e = entries();
    return e.length > 0 && index() < e.length;
  },

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

export function __resetErrorModalStoreForTests(): void {
  if (import.meta.env.MODE !== "test") return;
  setEntries([]);
  setIndex(0);
}
