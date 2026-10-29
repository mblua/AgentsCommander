import {
  Component,
  Show,
  createEffect,
  createMemo,
  createSignal,
  on,
  onCleanup,
  onMount,
  untrack,
} from "solid-js";
import { WebBrowserAPI, type WebBrowserBounds } from "../../shared/ipc";
import { webBrowserStore } from "../stores/webBrowser";

// #3367 — the page itself is a native window the backend lays over
// `.web-browser-viewport`; this component draws the toolbar and reports the
// viewport rectangle. A native window always paints above the DOM, so it is
// hidden while any app overlay or menu overlaps the viewport.
const FLOATING_SELECTOR = [
  ".modal-overlay",
  ".confirm-overlay",
  ".lock-conflict-overlay",
  ".agent-update-overlay",
  ".ctx-files-overlay",
  ".spec-board-modal-overlay",
  ".screenshot-overlay",
  ".action-bar-menu",
  ".layout-dropdown",
  ".session-context-menu",
  ".session-context-flyout",
  ".ac-wg-header-blocked-menu",
].join(", ");

function overlaps(a: DOMRect, b: DOMRect): boolean {
  return a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
}

/** CSS-pixel rect → physical pixels; devicePixelRatio folds in OS scale and zoom. */
export function physicalBounds(rect: DOMRect, ratio: number): WebBrowserBounds {
  return {
    x: rect.left * ratio,
    y: rect.top * ratio,
    width: rect.width * ratio,
    height: rect.height * ratio,
  };
}

export function isCovered(viewport: DOMRect, floating: Iterable<Element>): boolean {
  for (const el of floating) {
    if (overlaps(viewport, el.getBoundingClientRect())) return true;
  }
  return false;
}

const WebBrowserView: Component = () => {
  let viewportRef: HTMLDivElement | undefined;
  let inputRef: HTMLInputElement | undefined;
  let frame = 0;
  // Rounded bounds last sent to the backend; null while hidden.
  let shown: string | null = null;
  let disposed = false;
  const [draft, setDraft] = createSignal("");
  // True while the URL bar holds text the user typed and has not submitted.
  const [dirty, setDirty] = createSignal(false);
  const pageUrl = createMemo(() => webBrowserStore.url);
  const showPage = () => setDraft(pageUrl() === "about:blank" ? "" : pageUrl());

  // Follow page changes unless the user has typed. Blur must keep the text: a
  // mouse click on Go blurs the field before the form submits.
  createEffect(
    on(pageUrl, () => {
      if (!untrack(dirty)) showPage();
    }),
  );

  const run = (call: Promise<unknown>) => {
    call
      .then((result) => {
        if (result && typeof result === "object" && "url" in result) {
          webBrowserStore.apply(result as Parameters<typeof webBrowserStore.apply>[0]);
        }
      })
      .catch((e) => webBrowserStore.fail(e));
  };

  // A user action replaces the previous error; show/hide never clear it.
  const act = (start: () => Promise<unknown>) => {
    webBrowserStore.clearError();
    run(start());
  };

  const sync = () => {
    frame = 0;
    if (disposed || !viewportRef) return;
    const rect = viewportRef.getBoundingClientRect();
    const covered = isCovered(rect, document.querySelectorAll(FLOATING_SELECTOR));
    if (covered || rect.width < 1 || rect.height < 1) {
      if (shown !== null) {
        shown = null;
        run(WebBrowserAPI.hide());
      }
      return;
    }
    // The observers fire on any DOM change in the app; only real moves reach
    // the backend.
    const bounds = physicalBounds(rect, window.devicePixelRatio || 1);
    const key = [bounds.x, bounds.y, bounds.width, bounds.height].map(Math.round).join(",");
    if (key === shown) return;
    shown = key;
    run(WebBrowserAPI.show(bounds));
  };

  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(sync);
  };

  onMount(() => {
    const resize = new ResizeObserver(schedule);
    if (viewportRef) resize.observe(viewportRef);
    const mutations = new MutationObserver(schedule);
    mutations.observe(document.body, { childList: true, subtree: true });
    window.addEventListener("resize", schedule);
    schedule();
    onCleanup(() => {
      disposed = true;
      resize.disconnect();
      mutations.disconnect();
      window.removeEventListener("resize", schedule);
      if (frame) cancelAnimationFrame(frame);
      run(WebBrowserAPI.hide());
    });
  });

  const submit = (e: Event) => {
    e.preventDefault();
    const url = draft().trim();
    if (!url) return;
    setDirty(false);
    inputRef?.blur();
    act(() => WebBrowserAPI.navigate(url));
  };

  return (
    <div class="web-browser" data-ac-testid="main.webBrowser" data-ac-role="surface">
      <form class="web-browser-toolbar" onSubmit={submit}>
        <button
          type="button"
          class="web-browser-btn"
          title="Back"
          aria-label="Back"
          onClick={() => act(() => WebBrowserAPI.go(-1))}
          data-ac-testid="webBrowser.back"
          data-ac-role="button"
        >
          &#x2190;
        </button>
        <button
          type="button"
          class="web-browser-btn"
          title="Forward"
          aria-label="Forward"
          onClick={() => act(() => WebBrowserAPI.go(1))}
          data-ac-testid="webBrowser.forward"
          data-ac-role="button"
        >
          &#x2192;
        </button>
        <button
          type="button"
          class="web-browser-btn"
          classList={{ loading: webBrowserStore.loading }}
          title="Refresh"
          aria-label="Refresh"
          onClick={() => act(() => WebBrowserAPI.reload())}
          data-ac-testid="webBrowser.reload"
          data-ac-role="button"
          data-ac-state={webBrowserStore.loading ? "loading" : "idle"}
        >
          &#x21BB;
        </button>
        <input
          ref={inputRef}
          class="web-browser-url"
          type="text"
          spellcheck={false}
          placeholder="Enter a URL or an absolute file path"
          aria-label="URL"
          title={webBrowserStore.title || webBrowserStore.url}
          value={draft()}
          onInput={(e) => {
            setDraft(e.currentTarget.value);
            setDirty(true);
          }}
          onKeyDown={(e) => {
            if (e.key !== "Escape") return;
            setDirty(false);
            showPage();
            e.currentTarget.blur();
          }}
          data-ac-testid="webBrowser.url"
          data-ac-role="input"
        />
        <button
          type="submit"
          class="web-browser-go"
          title="Go"
          aria-label="Go"
          data-ac-testid="webBrowser.go"
          data-ac-role="button"
        >
          Go
        </button>
      </form>
      <Show when={webBrowserStore.error}>
        <div class="web-browser-error" role="alert" data-ac-testid="webBrowser.error">
          {webBrowserStore.error}
        </div>
      </Show>
      <div ref={viewportRef} class="web-browser-viewport" data-ac-testid="webBrowser.viewport" />
    </div>
  );
};

export default WebBrowserView;
