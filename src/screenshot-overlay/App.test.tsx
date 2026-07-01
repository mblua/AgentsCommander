// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  waitFor,
} from "../shared/testing/ui-harness";
import { FakeTransport } from "../shared/testing/fake-transport";
import type { ScreenshotOverlayState } from "../shared/types";
import ScreenshotOverlayApp from "./App";

// jsdom/vitest cannot render canvas PIXELS (getContext is a stub here), so this
// suite verifies the overlay WIRING: param parsing, state fetch, canvas sizing,
// and the pointer→confirm / Escape→cancel IPC flow. Pixel fidelity (frozen
// image, rubber-band, magnifier) is covered by the headless-Chrome check and
// manual Windows verification.

const IMG_W = 400;
const IMG_H = 300;
// 1x1 transparent PNG; decode() rejects under jsdom (the App swallows that and
// draws best-effort), which is fine — we assert wiring, not pixels.
const DATA_URL =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M8AAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

function overlayState(over: Partial<ScreenshotOverlayState> = {}): ScreenshotOverlayState {
  return {
    captureId: "abc-123",
    monitorId: 0,
    monitorX: 0,
    monitorY: 0,
    width: IMG_W,
    height: IMG_H,
    scaleFactor: 1,
    imageDataUrl: DATA_URL,
    sessionId: "s1",
    sessionName: "wg-6-dev-team/dev-webpage-ui",
    targetDirectory: "C:\\proj\\.ac\\wg-6-dev-team\\__agent_x",
    ...over,
  };
}

function pointer(type: string, x: number, y: number): Event {
  const e = new Event(type, { bubbles: true, cancelable: true });
  Object.assign(e, { clientX: x, clientY: y, pointerId: 1 });
  return e;
}

function stubRect(canvas: HTMLCanvasElement, width: number, height: number): void {
  // jsdom returns an all-zero rect; give the canvas a real on-screen size so the
  // proportional client→image mapping produces non-zero coordinates. Sized 1:1
  // with the image here for easy arithmetic.
  canvas.getBoundingClientRect = () =>
    ({
      left: 0,
      top: 0,
      right: width,
      bottom: height,
      width,
      height,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    }) as DOMRect;
}

describe("ScreenshotOverlayApp", () => {
  let restoreStubs: () => void;
  let cleanup: (() => void) | null = null;
  let originalSearch: string;
  let originalFocus: typeof window.focus;

  beforeEach(() => {
    restoreStubs = installBrowserDomStubs();
    // jsdom does not implement window.focus (it logs "Not implemented" noise);
    // the overlay calls it for real-browser ESC focus and already guards it, so
    // stub it to a no-op here.
    originalFocus = window.focus;
    window.focus = () => {};
    originalSearch = window.location.search;
    window.history.replaceState(
      {},
      "",
      "?window=screenshot-overlay&captureId=abc-123&monitorId=0"
    );
  });

  afterEach(() => {
    cleanup?.();
    cleanup = null;
    window.history.replaceState({}, "", originalSearch || "/");
    window.focus = originalFocus;
    restoreStubs();
  });

  async function mount(fake: FakeTransport): Promise<HTMLCanvasElement> {
    const rendered = renderWithFakeTransport(() => <ScreenshotOverlayApp />, fake);
    cleanup = rendered.cleanup;
    const canvas = rendered.root.querySelector(
      '[data-ac-testid="screenshotOverlay.canvas"]'
    ) as HTMLCanvasElement;
    expect(canvas).toBeTruthy();
    stubRect(canvas, IMG_W, IMG_H);
    // Wait for the async getOverlayState → canvas sizing to complete.
    await waitFor(() => expect(canvas.width).toBe(IMG_W));
    expect(canvas.height).toBe(IMG_H);
    return canvas;
  }

  it("fetches overlay state with the parsed captureId + numeric monitorId", async () => {
    const fake = new FakeTransport();
    fake.resolve("screenshot_get_overlay_state", overlayState());
    fake.resolve("screenshot_cancel_capture", undefined);

    await mount(fake);

    const call = fake.lastCall("screenshot_get_overlay_state");
    expect(call?.args).toEqual({ captureId: "abc-123", monitorId: 0 });
  });

  it("confirms the physical crop on a drag release", async () => {
    const fake = new FakeTransport();
    fake.resolve("screenshot_get_overlay_state", overlayState());
    fake.resolve("screenshot_confirm_selection", {
      path: "C:\\out\\shot.png",
      sessionId: "s1",
      sessionName: "wg-6-dev-team/dev-webpage-ui",
    });
    fake.resolve("screenshot_cancel_capture", undefined);

    const canvas = await mount(fake);
    canvas.dispatchEvent(pointer("pointerdown", 10, 20));
    canvas.dispatchEvent(pointer("pointermove", 210, 170));
    canvas.dispatchEvent(pointer("pointerup", 210, 170));

    await waitFor(() =>
      expect(fake.callsFor("screenshot_confirm_selection")).toHaveLength(1)
    );
    const selection = fake.lastCall("screenshot_confirm_selection")?.args
      .selection as Record<string, number>;
    expect(selection).toEqual({
      captureId: "abc-123",
      monitorId: 0,
      x: 10,
      y: 20,
      width: 200,
      height: 150,
    });
    // No cancel should fire once confirmed (settled guard).
    expect(fake.callsFor("screenshot_cancel_capture")).toHaveLength(0);
  });

  it("treats a sub-2px release as a mis-click: no confirm, overlay stays open", async () => {
    const fake = new FakeTransport();
    fake.resolve("screenshot_get_overlay_state", overlayState());
    fake.resolve("screenshot_confirm_selection", {
      path: "x",
      sessionId: "s1",
      sessionName: "n",
    });
    fake.resolve("screenshot_cancel_capture", undefined);

    const canvas = await mount(fake);
    canvas.dispatchEvent(pointer("pointerdown", 100, 100));
    canvas.dispatchEvent(pointer("pointerup", 101, 101));

    await Promise.resolve();
    expect(fake.callsFor("screenshot_confirm_selection")).toHaveLength(0);
    // Still open: neither confirmed nor cancelled by the tiny release.
    expect(fake.callsFor("screenshot_cancel_capture")).toHaveLength(0);
  });

  it("cancels on Escape", async () => {
    const fake = new FakeTransport();
    fake.resolve("screenshot_get_overlay_state", overlayState());
    fake.resolve("screenshot_cancel_capture", undefined);

    await mount(fake);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));

    await waitFor(() =>
      expect(fake.callsFor("screenshot_cancel_capture")).toHaveLength(1)
    );
    expect(fake.lastCall("screenshot_cancel_capture")?.args).toEqual({
      captureId: "abc-123",
    });
  });

  it("does not confirm after Escape has settled the capture", async () => {
    const fake = new FakeTransport();
    fake.resolve("screenshot_get_overlay_state", overlayState());
    fake.resolve("screenshot_confirm_selection", {
      path: "x",
      sessionId: "s1",
      sessionName: "n",
    });
    fake.resolve("screenshot_cancel_capture", undefined);

    const canvas = await mount(fake);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitFor(() =>
      expect(fake.callsFor("screenshot_cancel_capture")).toHaveLength(1)
    );

    // A late drag must be ignored once settled.
    canvas.dispatchEvent(pointer("pointerdown", 10, 10));
    canvas.dispatchEvent(pointer("pointerup", 200, 200));
    await Promise.resolve();
    expect(fake.callsFor("screenshot_confirm_selection")).toHaveLength(0);
  });
});
