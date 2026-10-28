import { createSignal } from "solid-js";
import type { WebBrowserSnapshot } from "../../shared/ipc";

// #3367 — last page state reported by the backend for the embedded web
// browser (URL, title, loading). Which central view is shown lives in
// centralView.ts; this store only mirrors the page.
const BLANK: WebBrowserSnapshot = { url: "about:blank", title: "", loading: false, visible: false };

const [snapshot, setSnapshot] = createSignal<WebBrowserSnapshot>(BLANK);
const [error, setError] = createSignal<string | null>(null);

export const webBrowserStore = {
  get url() {
    return snapshot().url;
  },
  get title() {
    return snapshot().title;
  },
  get loading() {
    return snapshot().loading;
  },
  get error() {
    return error();
  },

  apply(next: WebBrowserSnapshot) {
    setSnapshot(next);
    setError(null);
  },

  fail(e: unknown) {
    setError(e instanceof Error ? e.message : String(e));
  },
};

export function __resetWebBrowserStoreForTests() {
  if (import.meta.env.MODE !== "test") return;
  setSnapshot(BLANK);
  setError(null);
}
