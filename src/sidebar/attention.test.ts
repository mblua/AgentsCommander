// @vitest-environment jsdom
// vitest.config.ts sets `environment: 'node'` for the whole suite, and tests 8
// and 9 spy on `document.hasFocus`, which has no `document` under node.
import { afterEach, describe, expect, it, vi } from "vitest";

// The mocked exports are GETTERS over this mutable state, not plain values.
// Measured on vitest 4.1.5: a `vi.mock` factory runs ONCE per test file, a
// `vi.doMock` on the same specifier does not override it, `vi.doUnmock` drops
// the hoisted `vi.mock` too (so the REAL platform module loads and `isTauri`
// goes false for every later test), and `vi.resetModules()` does not re-run the
// factory. Getters are the only thing that varies per test here.
const mocks = vi.hoisted(() => ({
  requestUserAttention: vi.fn(),
  UserAttentionType: { Critical: "Critical", Informational: "Informational" },
  isTauri: true,
  windowModuleFails: false,
}));

vi.mock("../shared/platform", () => ({
  get isTauri() {
    return mocks.isTauri;
  },
  get isBrowser() {
    return !mocks.isTauri;
  },
  isWindows: true,
}));

vi.mock("@tauri-apps/api/window", () => ({
  get getCurrentWindow() {
    // Test 11: the `@tauri-apps/api/window` load failing. The throw lands on the
    // destructuring inside `requestTaskbarAttention`'s `try`, which is the same
    // catch a rejected `import()` would reach. A factory that rejects outright
    // cannot be used: it runs once for the whole file and would break tests 8-10.
    if (mocks.windowModuleFails) throw new Error("module unavailable");
    return () => ({ requestUserAttention: mocks.requestUserAttention });
  },
  UserAttentionType: mocks.UserAttentionType,
}));

import { requestTaskbarAttention } from "./attention";

afterEach(() => {
  vi.restoreAllMocks();
  mocks.requestUserAttention.mockReset();
  mocks.isTauri = true;
  mocks.windowModuleFails = false;
});

describe("requestTaskbarAttention (#1857)", () => {
  it("8. asks for Critical attention when the window is NOT focused", async () => {
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    await requestTaskbarAttention();
    expect(mocks.requestUserAttention).toHaveBeenCalledTimes(1);
    expect(mocks.requestUserAttention).toHaveBeenCalledWith(mocks.UserAttentionType.Critical);
  });

  it("9. does nothing when the window IS focused", async () => {
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    await requestTaskbarAttention();
    expect(mocks.requestUserAttention).not.toHaveBeenCalled();
  });

  it("10. does nothing outside Tauri, and does not throw", async () => {
    mocks.isTauri = false;
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    await expect(requestTaskbarAttention()).resolves.toBeUndefined();
    expect(mocks.requestUserAttention).not.toHaveBeenCalled();
  });

  it("11. resolves when the Tauri window module fails to load", async () => {
    mocks.windowModuleFails = true;
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    // Degrades to no flash: resolves, does not throw into the caller, and the
    // window API is never reached.
    await expect(requestTaskbarAttention()).resolves.toBeUndefined();
    expect(mocks.requestUserAttention).not.toHaveBeenCalled();
    expect(consoleError).toHaveBeenCalled();
  });
});
