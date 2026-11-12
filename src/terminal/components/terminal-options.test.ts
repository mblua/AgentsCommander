// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { EXTERNAL_LINK_REQUEST_EVENT } from "../../shared/external-links";
import { createTerminalOptions } from "./terminal-options";

describe("createTerminalOptions", () => {
  it("omits the custom xterm link handler in browser mode", () => {
    expect(createTerminalOptions(false).linkHandler).toBeUndefined();
  });

  it("installs a confirming xterm link handler in Tauri mode", () => {
    const options = createTerminalOptions(true);
    expect(options.linkHandler?.allowNonHttpProtocols).toBe(false);

    const listener = vi.fn();
    document.addEventListener(EXTERNAL_LINK_REQUEST_EVENT, listener);

    const event = {
      preventDefault: vi.fn(),
      stopPropagation: vi.fn(),
      stopImmediatePropagation: vi.fn(),
    } as unknown as MouseEvent;

    options.linkHandler?.activate(event, "https://example.com/docs", {
      start: { x: 1, y: 1 },
      end: { x: 1, y: 1 },
    });

    expect(event.preventDefault).toHaveBeenCalledOnce();
    expect(event.stopPropagation).toHaveBeenCalledOnce();
    expect(event.stopImmediatePropagation).toHaveBeenCalledOnce();
    expect(listener).toHaveBeenCalledWith(
      expect.objectContaining({
        detail: { url: "https://example.com/docs" },
      })
    );

    document.removeEventListener(EXTERNAL_LINK_REQUEST_EVENT, listener);
  });
});

const INSTALLED = new Set(["JetBrains Mono"]);
const fakeWidth = (font: string, text: string): number => {
  for (const family of INSTALLED) if (font.includes(`'${family}'`)) return 8.4;
  return text === "l" || text === "i" ? 3.11 : 13.21;
};

describe("createTerminalOptions fontFamily (#4244)", () => {
  afterEach(() => vi.restoreAllMocks());

  it("drops preferred fonts that do not measure monospace", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
      font: "",
      measureText(this: { font: string }, t: string) {
        return { width: fakeWidth(this.font, t) };
      },
    } as unknown as CanvasRenderingContext2D);
    expect(createTerminalOptions(false).fontFamily).toBe("'JetBrains Mono', monospace");
  });
});
