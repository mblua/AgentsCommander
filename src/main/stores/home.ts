import { createSignal } from "solid-js";
import { HomeAPI } from "../../shared/ipc";

const [visible, setVisible] = createSignal(false);
const [content, setContent] = createSignal<string | null>(null);
const [loading, setLoading] = createSignal(false);
const [error, setError] = createSignal<string | null>(null);

export const homeStore = {
  get visible() { return visible(); },
  get content() { return content(); },
  get loading() { return loading(); },
  get error() { return error(); },

  // Legacy explicit initializer; callers should reason in selection modes.
  // After boot, visibility is user-controlled.
  setInitialVisibility(hasActiveSession: boolean) {
    setVisible(!hasActiveSession);
  },

  toggle() { setVisible((v) => !v); },
  show() { setVisible(true); },
  hide() { setVisible(false); },

  // Idempotent. Sets loading + error appropriately.
  async fetch() {
    if (loading()) return;
    setLoading(true);
    setError(null);
    try {
      const text = await HomeAPI.fetchMarkdown();
      setContent(text);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
    } finally {
      setLoading(false);
    }
  },

  // Manual refresh — re-runs fetch but does NOT wipe currently-displayed
  // content. If the refetch fails, the user keeps seeing the last-good content
  // (regression guard for plan §A1 / Grinch finding #3).
  async refresh() {
    setError(null);
    await this.fetch();
  },
};

// Test-only reset (gated to Vite MODE === "test", which vitest sets). Resets
// every signal so vitest tests do not leak state across cases.
export function __resetHomeStoreForTests() {
  if (import.meta.env.MODE !== "test") return;
  setVisible(false);
  setContent(null);
  setLoading(false);
  setError(null);
}
