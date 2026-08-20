// @vitest-environment jsdom
//
// #1363 - the half of listener scoping that only this layer can pin.
//
// `TerminalView.attachment.test.tsx` proves the app ASKS for a window-scoped
// `pty_output` listener; it runs against a FakeTransport, so it cannot prove
// the request becomes a concrete window label on the wire. This does.
//
// Why it matters: Tauri short-circuits the `emit_to` label filter for any
// listener whose `EventTarget` is `Any` (`tauri-2.10.3/src/event/listener.rs:
// 306-311`), and `Any` is what `@tauri-apps/api`'s `listen()` registers when no
// target is given (`event.js:69-73`). A dropped target is therefore invisible
// downstream — no wrong byte is ever written — and only the registration
// itself can catch it.
import { beforeEach, describe, expect, it, vi } from "vitest";

const WINDOW_LABEL = "terminal-window-1";

const tauri = vi.hoisted(() => ({
  listenCalls: [] as { event: string; options: unknown }[],
  labelThrows: false,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, _handler: unknown, options?: unknown) => {
    tauri.listenCalls.push({ event, options });
    return () => undefined;
  }),
  emit: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => {
    if (tauri.labelThrows) {
      throw new TypeError("Cannot read properties of undefined (reading 'metadata')");
    }
    return { label: WINDOW_LABEL };
  },
}));

import { TauriTransport } from "./transport-tauri";

describe("TauriTransport listener scoping (#1363)", () => {
  beforeEach(() => {
    tauri.listenCalls.length = 0;
    tauri.labelThrows = false;
  });

  it("turns scopeToCurrentWindow into this webview's label", async () => {
    const transport = new TauriTransport();

    await transport.listen("pty_output", () => {}, { scopeToCurrentWindow: true });

    expect(tauri.listenCalls).toEqual([
      { event: "pty_output", options: { target: WINDOW_LABEL } },
    ]);
  });

  // Every other event in the app is delivered with a plain `emit`, which passes
  // no filter, and with no filter every handler matches whatever its target is
  // (`listener.rs:296-302`). Those listeners must stay unscoped.
  it("leaves an unscoped listener unscoped", async () => {
    const transport = new TauriTransport();

    await transport.listen("session_created", () => {});
    await transport.listen("session_destroyed", () => {}, {});

    expect(tauri.listenCalls).toEqual([
      { event: "session_created", options: undefined },
      { event: "session_destroyed", options: undefined },
    ]);
  });

  // The label lookup must never reject `init()`: `this.ready` is awaited by
  // invoke, listen and emit alike, so a throw there would take the whole
  // transport down. Degrading to the unscoped registration the app shipped
  // before #1363 costs bandwidth; a dead transport is a dead window.
  it("still registers, unscoped, when the label cannot be resolved", async () => {
    tauri.labelThrows = true;
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const transport = new TauriTransport();

    await transport.listen("pty_output", () => {}, { scopeToCurrentWindow: true });
    // Not merely "listen worked": the rest of the transport is alive too.
    await expect(transport.invoke("get_settings")).resolves.toBeUndefined();

    expect(tauri.listenCalls).toEqual([{ event: "pty_output", options: undefined }]);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
