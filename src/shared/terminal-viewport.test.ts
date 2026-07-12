// @vitest-environment jsdom
//
// #973 — the guard rails around the size we hand to `create_session`.
//
// dev-rust measured the fix as it will actually ship: opening the PTY at the
// fitted size and letting the view's redundant same-size burst land is 0/10
// blank, but letting it land ONE ROW off is 6/10 blank. So the only thing worse
// than not sending a size is sending a wrong one, and every rule below exists to
// make sure a wrong one can never be sent or — just as bad — recorded.
//
// Recording matters as much as sending: `TerminalView` dedups resizes against
// the size it believes the PTY was opened at. Record a size the backend did not
// honour and the corrective resize is skipped as a no-op, wedging that PTY at
// the wrong size for its whole life.
//
// This file runs WITHOUT a platform mock, so `isTauri` is false and `isBrowser`
// is true — which is exactly the environment the browser-mode test needs.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SessionAPI, __setTransportForTests } from "./ipc";
import { FakeTransport } from "./testing/fake-transport";
import { session } from "./testing/ui-harness";
import {
  measurePtyViewport,
  registerPtyViewportProbe,
  rememberSpawnViewport,
  resetPtyViewportForTests,
  takeSpawnViewport,
} from "./terminal-viewport";

describe("pty spawn viewport (#973)", () => {
  let restoreTransport: (() => void) | null = null;

  beforeEach(() => {
    resetPtyViewportForTests();
  });

  afterEach(() => {
    restoreTransport?.();
    restoreTransport = null;
    resetPtyViewportForTests();
    vi.restoreAllMocks();
  });

  describe("measurePtyViewport", () => {
    it("returns null when no terminal has registered a probe", () => {
      expect(measurePtyViewport()).toBeNull();
    });

    it("returns the probed size when a terminal is on screen to measure", () => {
      registerPtyViewportProbe(() => ({ cols: 74, rows: 23 }));
      expect(measurePtyViewport()).toEqual({ cols: 74, rows: 23 });
    });

    it("returns null once the terminal that registered the probe is gone", () => {
      const unregister = registerPtyViewportProbe(() => ({ cols: 74, rows: 23 }));
      unregister();
      expect(measurePtyViewport()).toBeNull();
    });

    // xterm's fit genuinely returns a zero dimension while its container is still
    // being laid out. The backend's `PtyViewport::from_fit` SILENTLY falls back to
    // 120x30 on a zero — so sending one would open the PTY at a size we then
    // recorded as 0xN and deduped every corrective resize against. Never send it.
    it.each([
      ["zero cols", { cols: 0, rows: 23 }],
      ["zero rows", { cols: 74, rows: 0 }],
      ["negative", { cols: -1, rows: 23 }],
      ["fractional", { cols: 74.5, rows: 23 }],
      ["NaN", { cols: Number.NaN, rows: 23 }],
      ["beyond u16", { cols: 70000, rows: 23 }],
    ])("returns null for a degenerate fit: %s", (_label, viewport) => {
      registerPtyViewportProbe(() => viewport);
      expect(measurePtyViewport()).toBeNull();
    });

    it("returns null, and does not throw, when the probe itself throws", () => {
      vi.spyOn(console, "warn").mockImplementation(() => {});
      registerPtyViewportProbe(() => {
        throw new Error("not laid out");
      });

      // A DOM read must never be able to fail a session create.
      expect(measurePtyViewport()).toBeNull();
    });
  });

  describe("spawn viewport memory", () => {
    it("hands the recorded size to the first terminal built for the session", () => {
      rememberSpawnViewport("session-1", { cols: 74, rows: 23 });
      expect(takeSpawnViewport("session-1")).toEqual({ cols: 74, rows: 23 });
    });

    // Consumed on read: it is only true for the FIRST terminal. A re-attach — a
    // detached window, a re-created tile — must fit and resize normally, because
    // the child has long since started and the tile may be a different size now.
    it("does not hand the same spawn size to a re-attach", () => {
      rememberSpawnViewport("session-1", { cols: 74, rows: 23 });
      takeSpawnViewport("session-1");
      expect(takeSpawnViewport("session-1")).toBeNull();
    });

    it("returns null for a session that was never given a size", () => {
      expect(takeSpawnViewport("never-created")).toBeNull();
    });

    it("refuses to record a size the backend would not have honoured", () => {
      rememberSpawnViewport("session-1", { cols: 0, rows: 23 });
      expect(takeSpawnViewport("session-1")).toBeNull();
    });
  });

  describe("SessionAPI.create", () => {
    const mountTransport = (): FakeTransport => {
      const fake = new FakeTransport();
      fake.resolve("create_session", session({ id: "created" }));
      restoreTransport = __setTransportForTests(fake);
      return fake;
    };

    // Browser mode: the web-mode `create_session` handler parses its args by hand
    // and ignores cols/rows entirely — it always spawns at 120x30. Sending a size
    // it will not honour, and then recording it, would make the terminal dedup the
    // corrective resize away and strand that PTY at 120x30 forever.
    it("sends no size in browser mode, even with a terminal on screen to measure", async () => {
      const fake = mountTransport();
      registerPtyViewportProbe(() => ({ cols: 74, rows: 23 }));

      await SessionAPI.create({ cwd: "C:\\Project" });

      const args = fake.lastCall("create_session")!.args;
      expect(args.cols).toBeNull();
      expect(args.rows).toBeNull();
    });

    it("records nothing for a session it sent no size for", async () => {
      mountTransport();
      registerPtyViewportProbe(() => ({ cols: 74, rows: 23 }));

      await SessionAPI.create({ cwd: "C:\\Project" });

      // Browser mode measured nothing, so the terminal must be free to fit and
      // resize this PTY normally.
      expect(takeSpawnViewport("created")).toBeNull();
    });
  });
});
