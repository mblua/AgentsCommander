import { createSignal } from "solid-js";

// #552 coarse "now" tick for live-updating elapsed-time badges (the coordinator
// idle badge). A single app-lifetime module singleton — NOT a per-row timer —
// so every badge memo that reads `clockStore.nowMs` re-runs together once per
// cadence. 30s keeps the minutes-granular badge within ~30s of accurate without
// busy work. The interval is intentionally never cleared (module-lifetime), the
// same pattern other module singletons in this app use.
const [nowMs, setNowMs] = createSignal(Date.now());
setInterval(() => setNowMs(Date.now()), 30_000);

export const clockStore = {
  get nowMs() {
    return nowMs();
  },
};
