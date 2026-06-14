import { describe, expect, it, vi } from "vitest";
import { FakeTransport } from "./fake-transport";

describe("FakeTransport", () => {
  it("records invoke payloads before resolving", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", { ok: true });

    await expect(fake.invoke("get_settings", { value: 1 })).resolves.toEqual({
      ok: true,
    });

    expect(fake.calls).toEqual([
      { cmd: "get_settings", args: { value: 1 } },
    ]);
  });

  it("fails loudly for unhandled invokes", async () => {
    const fake = new FakeTransport();

    await expect(fake.invoke("missing_command")).rejects.toThrow(
      "Unhandled fake transport invoke: missing_command"
    );
    expect(fake.lastCall("missing_command")?.args).toEqual({});
  });

  it("delivers backend events and removes listeners", async () => {
    const fake = new FakeTransport();
    const listener = vi.fn();
    const unlisten = await fake.listen("session_switched", listener);

    fake.emitFromBackend("session_switched", { id: "session-1" });
    unlisten();
    fake.emitFromBackend("session_switched", { id: "session-2" });

    expect(listener).toHaveBeenCalledOnce();
    expect(listener).toHaveBeenCalledWith({ id: "session-1" });
  });

  it("records broadcast events without requiring a handler", async () => {
    const fake = new FakeTransport();

    await expect(fake.emit("open_settings", { section: "integrations" })).resolves.toBeUndefined();

    expect(fake.lastCall("broadcast_event")).toEqual({
      cmd: "broadcast_event",
      args: {
        event: "open_settings",
        payload: { section: "integrations" },
      },
    });
  });

  it("records binary PTY writes only when enabled", () => {
    const jsonOnly = new FakeTransport();
    expect(jsonOnly.writePtyBinary).toBeUndefined();

    const binary = new FakeTransport({ binaryPty: true });
    binary.writePtyBinary?.("session-1", new Uint8Array([65, 66]));

    expect(binary.binaryWrites).toEqual([
      { sessionId: "session-1", data: [65, 66] },
    ]);
    expect(binary.calls).toEqual([]);
  });
});
