// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render } from "solid-js/web";
import WebBrowserView, { isCovered, physicalBounds } from "./WebBrowserView";

const api = vi.hoisted(() => {
  const snapshot = { url: "about:blank", title: "", loading: false, visible: true };
  return {
    show: vi.fn(async () => snapshot),
    hide: vi.fn(async () => snapshot),
    navigate: vi.fn(async () => snapshot),
    go: vi.fn(async () => undefined),
    reload: vi.fn(async () => snapshot),
    snapshot: vi.fn(async () => snapshot),
  };
});

vi.mock("../../shared/ipc", () => ({ WebBrowserAPI: api }));

const rect = (left: number, top: number, width: number, height: number) =>
  ({ left, top, width, height, right: left + width, bottom: top + height }) as DOMRect;
const floating = (r: DOMRect) => ({ getBoundingClientRect: () => r }) as Element;

describe("WebBrowserView geometry (#3367)", () => {
  it("scales CSS pixels by the device pixel ratio", () => {
    expect(physicalBounds(rect(10, 20, 300, 200), 1.5)).toEqual({
      x: 15,
      y: 30,
      width: 450,
      height: 300,
    });
  });

  it("is covered only by floating elements that overlap the viewport", () => {
    const viewport = rect(100, 100, 400, 300);
    expect(isCovered(viewport, [])).toBe(false);
    expect(isCovered(viewport, [floating(rect(0, 0, 90, 90))])).toBe(false);
    expect(isCovered(viewport, [floating(rect(500, 100, 50, 50))])).toBe(false);
    expect(isCovered(viewport, [floating(rect(450, 350, 100, 100))])).toBe(true);
    expect(isCovered(viewport, [floating(rect(0, 0, 2000, 2000))])).toBe(true);
  });
});

describe("WebBrowserView Go button (#3367)", () => {
  it("navigates to the typed URL like Enter does", () => {
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
      },
    );
    const root = document.createElement("div");
    document.body.appendChild(root);
    const dispose = render(() => WebBrowserView({}), root);
    try {
      const url = root.querySelector<HTMLInputElement>('[data-ac-testid="webBrowser.url"]')!;
      url.value = "  example.com  ";
      url.dispatchEvent(new Event("input", { bubbles: true }));
      root.querySelector<HTMLButtonElement>('[data-ac-testid="webBrowser.go"]')!.click();
      expect(api.navigate).toHaveBeenCalledWith("example.com");
    } finally {
      dispose();
      root.remove();
      vi.unstubAllGlobals();
    }
  });
});
