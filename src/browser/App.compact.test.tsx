// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  renderWithFakeTransport,
  resetSidebarCompactForTests,
} from "../shared/testing/ui-harness";
import {
  restoreWidthPx,
  setRestoreWidthPx,
  setSidebarCompactMode,
  sidebarCompact,
} from "../shared/sidebar-compact";

// #2280 — only BrowserApp's own compact host is under test here; the embedded
// sidebar and terminal are inert so the browser pane, divider and animation
// are what this file pins. No ipc mock: renderWithFakeTransport wires the real
// ipc module to a FakeTransport instead.
vi.mock("../sidebar/App", () => ({ default: () => null }));
vi.mock("../terminal/App", () => ({ default: () => null }));

import BrowserApp from "./App";

const COMPACT_WIDTH = "calc(var(--ac-rail-width) + 0px)";
// Vite rewrites the literal `new URL(..., import.meta.url)` form into a served
// asset URL (http://localhost:3000/...) under jsdom; the variable base keeps
// the real file: URL that node:fs accepts.
const moduleUrl = import.meta.url;
// jsdom applies no stylesheet, so the transition rule is pinned from the bytes
// (normalized once; a CRLF checkout would otherwise match nothing vacuously).
const BROWSER_CSS = readFileSync(
  new URL("./styles/browser.css", moduleUrl),
  "utf8",
).split("\r\n").join("\n");

type Rendered = ReturnType<typeof renderWithFakeTransport>;

function mountBrowserApp(): Rendered {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings({ mainSidebarSide: "right" }));
  return renderWithFakeTransport(() => <BrowserApp />, fake);
}

// Microtask-only settle: the animation test runs on fake timers, so a
// setTimeout-based wait would never fire there.
async function flushMicrotasks(): Promise<void> {
  for (let pass = 0; pass < 8; pass += 1) {
    await Promise.resolve();
  }
}

function pane(rendered: Rendered): HTMLElement {
  return rendered.root.querySelector(".browser-sidebar") as HTMLElement;
}

function divider(rendered: Rendered): HTMLElement {
  return rendered.root.querySelector(".browser-divider") as HTMLElement;
}

function paneWidth(rendered: Rendered): string {
  return pane(rendered).style.width;
}

// The host defaults to the right side, where the width is
// `window.innerWidth - clientX`; this is the clientX that lands on widthPx.
function clientXForWidth(widthPx: number): number {
  return window.innerWidth - widthPx;
}

function mouseAt(target: EventTarget, type: string, clientX: number): void {
  target.dispatchEvent(
    new MouseEvent(type, { bubbles: true, cancelable: true, clientX }),
  );
}

// jsdom has no Touch/TouchEvent constructor, so the one field the move handler
// reads is attached to a plain cancelable Event.
function touchAt(target: EventTarget, type: string, clientX?: number): void {
  const event = new Event(type, { bubbles: true, cancelable: true });
  if (clientX !== undefined) {
    Object.defineProperty(event, "touches", { value: [{ clientX }] });
  }
  target.dispatchEvent(event);
}

function dragMouseTo(rendered: Rendered, widthPx: number): void {
  const handle = divider(rendered);
  mouseAt(handle, "mousedown", clientXForWidth(300));
  mouseAt(document, "mousemove", clientXForWidth(widthPx));
  mouseAt(document, "mouseup", clientXForWidth(widthPx));
}

function dragTouchTo(rendered: Rendered, widthPx: number): void {
  const handle = divider(rendered);
  touchAt(handle, "touchstart", clientXForWidth(300));
  touchAt(document, "touchmove", clientXForWidth(widthPx));
  touchAt(document, "touchend");
}

describe("BrowserApp compact host (#2280)", () => {
  beforeEach(() => {
    resetSidebarCompactForTests();
  });

  afterEach(() => {
    resetSidebarCompactForTests();
    vi.useRealTimers();
    document.body.innerHTML = "";
  });

  it("renders the pane at the host's 300 default while expanded", async () => {
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();
      expect(sidebarCompact()).toBe(false);
      expect(paneWidth(rendered)).toBe("300px");
    } finally {
      rendered.cleanup();
    }
  });

  it("renders the pane at the rail token while compact", async () => {
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();
      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);
      expect(paneWidth(rendered)).toBe(COMPACT_WIDTH);
    } finally {
      rendered.cleanup();
    }
  });

  it("snapshots and restores 250 without overwriting the shared snapshot", async () => {
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();
      // Seed the shared snapshot with a sentinel. A host that used the shared
      // getter/setter would overwrite it with 250 at collapse time and still
      // restore a correct-looking 250 through its own [200, 600] clamp, so the
      // sentinel assertion after the collapse is what fails there.
      setRestoreWidthPx(517);
      dragMouseTo(rendered, 250);
      expect(paneWidth(rendered)).toBe("250px");

      setSidebarCompactMode(true);
      expect(paneWidth(rendered)).toBe(COMPACT_WIDTH);
      expect(restoreWidthPx()).toBe(517);

      setSidebarCompactMode(false);
      expect(paneWidth(rendered)).toBe("250px");
      expect(restoreWidthPx()).toBe(517);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps both compact starts inert via their own guards", async () => {
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();
      dragMouseTo(rendered, 250);

      setSidebarCompactMode(true);
      const handle = divider(rendered);
      const layout = rendered.root.querySelector(".browser-layout") as HTMLElement;
      expect(handle.getAttribute("aria-disabled")).toBe("true");
      expect(paneWidth(rendered)).toBe(COMPACT_WIDTH);

      // Each start handler has its own early return, so each modality is
      // proven separately at its own start event: an injected missing guard
      // would add browser-dragging and then move the hidden width.
      mouseAt(handle, "mousedown", clientXForWidth(250));
      expect(layout.classList.contains("browser-dragging")).toBe(false);
      mouseAt(document, "mousemove", clientXForWidth(370));
      expect(paneWidth(rendered)).toBe(COMPACT_WIDTH);
      mouseAt(document, "mouseup", clientXForWidth(370));

      touchAt(handle, "touchstart", clientXForWidth(250));
      expect(layout.classList.contains("browser-dragging")).toBe(false);
      touchAt(document, "touchmove", clientXForWidth(370));
      expect(paneWidth(rendered)).toBe(COMPACT_WIDTH);
      touchAt(document, "touchend");
      expect(paneWidth(rendered)).toBe(COMPACT_WIDTH);

      // The hidden signal stayed undragged, so the snapshot survives the flip.
      setSidebarCompactMode(false);
      expect(paneWidth(rendered)).toBe("250px");
    } finally {
      rendered.cleanup();
    }
  });

  it("positive control: the expanded divider still drags on mouse and touch", async () => {
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();

      dragMouseTo(rendered, 250);
      expect(paneWidth(rendered)).toBe("250px");
      dragMouseTo(rendered, 370);
      expect(paneWidth(rendered)).toBe("370px");

      dragTouchTo(rendered, 250);
      expect(paneWidth(rendered)).toBe("250px");
      dragTouchTo(rendered, 370);
      expect(paneWidth(rendered)).toBe("370px");
    } finally {
      rendered.cleanup();
    }
  });

  it("animates only a mode change, on the real pane, and disarms the fallback", async () => {
    vi.useFakeTimers();
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();
      const panel = pane(rendered);

      setSidebarCompactMode(true);
      expect(panel.classList.contains("ac-sidebar-animating")).toBe(true);
      expect(vi.getTimerCount()).toBe(1);
      panel.dispatchEvent(new Event("transitionend", { bubbles: true }));
      expect(panel.classList.contains("ac-sidebar-animating")).toBe(false);
      expect(vi.getTimerCount()).toBe(0);

      setSidebarCompactMode(false);
      expect(panel.classList.contains("ac-sidebar-animating")).toBe(true);
      await vi.advanceTimersByTimeAsync(400);
      expect(panel.classList.contains("ac-sidebar-animating")).toBe(false);
      expect(vi.getTimerCount()).toBe(0);

      // Drags are width writes, not mode changes.
      mouseAt(divider(rendered), "mousedown", clientXForWidth(300));
      mouseAt(document, "mousemove", clientXForWidth(260));
      expect(panel.classList.contains("ac-sidebar-animating")).toBe(false);
      mouseAt(document, "mouseup", clientXForWidth(260));

      touchAt(divider(rendered), "touchstart", clientXForWidth(260));
      touchAt(document, "touchmove", clientXForWidth(280));
      expect(panel.classList.contains("ac-sidebar-animating")).toBe(false);
      touchAt(document, "touchend");

      // The declarations and their single owner, from the bytes.
      expect(
        BROWSER_CSS.match(/transition:\s*width 180ms ease-out;/g) ?? [],
      ).toHaveLength(1);
      expect(BROWSER_CSS).toMatch(
        /\.browser-sidebar\.ac-sidebar-animating\s*\{\s*transition:\s*width 180ms ease-out;\s*\}/,
      );
      expect(BROWSER_CSS).toMatch(
        /@media \(prefers-reduced-motion: reduce\) \{\s*\.browser-sidebar\.ac-sidebar-animating\s*\{\s*transition:\s*none;\s*\}\s*\}/,
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps interleaved mouse/touch owners separate and ends both on a mode change", async () => {
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();
      const layout = rendered.root.querySelector(".browser-layout") as HTMLElement;
      const handle = divider(rendered);
      expect(paneWidth(rendered)).toBe("300px");

      // Overlapping starts: the later touch start must not overwrite the
      // mouse owner, so the mouse pair still moves the width.
      mouseAt(handle, "mousedown", clientXForWidth(300));
      touchAt(handle, "touchstart", clientXForWidth(300));
      expect(layout.classList.contains("browser-dragging")).toBe(true);

      mouseAt(document, "mousemove", clientXForWidth(250));
      expect(paneWidth(rendered)).toBe("250px");

      // The ordinary mouse end removes only the mouse pair; the touch owner
      // stays live, so browser-dragging must remain until the touchend.
      mouseAt(document, "mouseup", clientXForWidth(250));
      mouseAt(document, "mousemove", clientXForWidth(350));
      expect(paneWidth(rendered)).toBe("250px");
      expect(layout.classList.contains("browser-dragging")).toBe(true);

      touchAt(document, "touchmove", clientXForWidth(280));
      expect(paneWidth(rendered)).toBe("280px");
      touchAt(document, "touchend");
      expect(layout.classList.contains("browser-dragging")).toBe(false);

      // Both ordinary ends are done: neither modality may still move the pane.
      mouseAt(document, "mousemove", clientXForWidth(350));
      touchAt(document, "touchmove", clientXForWidth(350));
      expect(paneWidth(rendered)).toBe("280px");

      // A mode change with both saves active must end both before the flip, so
      // the remembered width survives and no stale listener can move it.
      mouseAt(handle, "mousedown", clientXForWidth(280));
      touchAt(handle, "touchstart", clientXForWidth(280));
      expect(layout.classList.contains("browser-dragging")).toBe(true);

      setSidebarCompactMode(true);
      expect(layout.classList.contains("browser-dragging")).toBe(false);
      expect(paneWidth(rendered)).toBe(COMPACT_WIDTH);

      mouseAt(document, "mousemove", clientXForWidth(500));
      touchAt(document, "touchmove", clientXForWidth(500));
      setSidebarCompactMode(false);
      expect(paneWidth(rendered)).toBe("280px");

      mouseAt(document, "mousemove", clientXForWidth(500));
      touchAt(document, "touchmove", clientXForWidth(500));
      expect(paneWidth(rendered)).toBe("280px");
    } finally {
      rendered.cleanup();
    }
  });

  it("re-bases a repeated mouse start so no mouse listener survives the end", async () => {
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();
      const layout = rendered.root.querySelector(".browser-layout") as HTMLElement;
      const handle = divider(rendered);
      expect(paneWidth(rendered)).toBe("300px");

      // A second mousedown must retire the first mouse pair before installing
      // its own; otherwise the first pair survives the mouseup below.
      mouseAt(handle, "mousedown", clientXForWidth(300));
      mouseAt(handle, "mousedown", clientXForWidth(300));
      expect(layout.classList.contains("browser-dragging")).toBe(true);

      mouseAt(document, "mousemove", clientXForWidth(250));
      expect(paneWidth(rendered)).toBe("250px");
      mouseAt(document, "mouseup", clientXForWidth(250));
      expect(layout.classList.contains("browser-dragging")).toBe(false);

      // The leak is only visible here: a stale first mouse pair resizes after
      // an end the slot believed had removed it.
      mouseAt(document, "mousemove", clientXForWidth(350));
      expect(paneWidth(rendered)).toBe("250px");
    } finally {
      rendered.cleanup();
    }
  });

  it("re-bases a repeated touch start so no touch listener survives the end", async () => {
    const rendered = mountBrowserApp();
    try {
      await flushMicrotasks();
      const layout = rendered.root.querySelector(".browser-layout") as HTMLElement;
      const handle = divider(rendered);
      expect(paneWidth(rendered)).toBe("300px");

      touchAt(handle, "touchstart", clientXForWidth(300));
      touchAt(handle, "touchstart", clientXForWidth(300));
      expect(layout.classList.contains("browser-dragging")).toBe(true);

      touchAt(document, "touchmove", clientXForWidth(250));
      expect(paneWidth(rendered)).toBe("250px");
      touchAt(document, "touchend");
      expect(layout.classList.contains("browser-dragging")).toBe(false);

      // Touch has its own re-entry hazard and its own stale pair to leave.
      touchAt(document, "touchmove", clientXForWidth(350));
      expect(paneWidth(rendered)).toBe("250px");
    } finally {
      rendered.cleanup();
    }
  });
});
