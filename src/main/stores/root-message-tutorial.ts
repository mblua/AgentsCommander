import { createSignal } from "solid-js";

// #5287 - the running "one message to the Root Agent" tutorial. Not saved.
const [active, setActive] = createSignal(false);

export const rootMessageStore = {
  get active(): boolean { return active(); },
  start(): void {
    if (active()) return;
    setActive(true);
  },
  exit(): void { setActive(false); },
};

// Test-only reset (gated to Vite MODE === "test", which vitest sets).
export function __resetRootMessageStoreForTests() {
  if (import.meta.env.MODE !== "test") return;
  setActive(false);
}
