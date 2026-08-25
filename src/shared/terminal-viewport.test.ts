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
  BACKEND_SPAWN_FLOOR,
  executeUiTerminalController,
  measurePtyViewport,
  registerPtyViewportProbe,
  registerUiTerminalController,
  rememberSpawnViewport,
  resetPtyViewportForTests,
  resetUiTerminalControllerForTests,
  takeSpawnViewport,
} from "./terminal-viewport";

/** The Rust half of the floor, read from source. See the drift guard below. */
const BACKEND_SOURCES = import.meta.glob<string>("../../src-tauri/src/**/*.rs", {
  query: "?raw",
  import: "default",
  eager: true,
});

describe("pty spawn viewport (#973)", () => {
  let restoreTransport: (() => void) | null = null;

  beforeEach(() => {
    resetPtyViewportForTests();
    resetUiTerminalControllerForTests();
  });

  afterEach(() => {
    restoreTransport?.();
    restoreTransport = null;
    resetPtyViewportForTests();
    resetUiTerminalControllerForTests();
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

    // A size the backend will not open the PTY at must never be sent, because we
    // would then RECORD it and dedup the corrective resize away against it.
    //
    // Two different hazards live in this list. The malformed ones (NaN, fractional,
    // out of u16 range) are what a broken probe could produce — xterm itself cannot
    // hand back a zero, it clamps to MINIMUM_COLS = 2 / MINIMUM_ROWS = 1, and the
    // worst it can propagate is NaN. The BELOW-FLOOR ones are the dangerous ones,
    // because they are perfectly well-formed: a collapsed-but-laid-out container
    // measures a real, honest 2x1, which sails straight through a `> 0` check while
    // `PtyViewport::from_fit` quietly opens the PTY at 120x30 instead.
    it.each([
      ["zero cols", { cols: 0, rows: 23 }],
      ["zero rows", { cols: 74, rows: 0 }],
      ["negative", { cols: -1, rows: 23 }],
      ["fractional", { cols: 74.5, rows: 23 }],
      ["NaN", { cols: Number.NaN, rows: 23 }],
      ["beyond u16", { cols: 70000, rows: 23 }],
      ["a collapsed box: xterm's own 2x1 clamp", { cols: 2, rows: 1 }],
      ["one column below the floor", { cols: 19, rows: 40 }],
      ["one row below the floor", { cols: 80, rows: 4 }],
    ])("returns null for a fit the backend would not honour: %s", (_label, viewport) => {
      registerPtyViewportProbe(() => viewport);
      expect(measurePtyViewport()).toBeNull();
    });

    // The floor is a floor, not a preference: a fit sitting exactly on it is a real
    // size the backend opens the PTY at, and refusing it would cost that session the
    // whole fix for nothing.
    it("returns a fit that sits exactly on the floor", () => {
      registerPtyViewportProbe(() => ({ ...BACKEND_SPAWN_FLOOR }));
      expect(measurePtyViewport()).toEqual(BACKEND_SPAWN_FLOOR);
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

    // The second door into the wedge, and the one that is NOT hypothetical. The
    // backend floors this to 120x30 without failing the spawn. Record the 2x1 and
    // the terminal starts at 2x1, its fit finds nothing to do, and the resize that
    // would have corrected the PTY is deduped away — leaving the PTY at 120x30
    // behind a terminal that is not that size, forever.
    it("refuses to record a below-floor size the backend silently opens at 120x30", () => {
      rememberSpawnViewport("session-1", { cols: 2, rows: 1 });
      expect(takeSpawnViewport("session-1")).toBeNull();
    });
  });

  // The two floors are one rule living in two languages, and a comment cannot keep
  // them honest. This reads the Rust and fails the moment they disagree: drift here
  // does not produce a type error or a failed call, it produces a stranded PTY.
  describe("the floor mirrors the backend", () => {
    it("matches PtyViewport::PLAUSIBLE_MIN in src-tauri/src/pty/backend.rs", () => {
      const pattern =
        /const PLAUSIBLE_MIN:\s*Self\s*=\s*Self\s*\{\s*cols:\s*(\d+)\s*,\s*rows:\s*(\d+)\s*,?\s*\}/g;

      const found: { cols: number; rows: number }[] = [];
      for (const source of Object.values(BACKEND_SOURCES)) {
        let match: RegExpExecArray | null;
        while ((match = pattern.exec(source)) !== null) {
          found.push({ cols: Number(match[1]), rows: Number(match[2]) });
        }
      }

      // Not found means the backend renamed or restructured the constant. Do not
      // paper over it: find the new floor and mirror it here, or this whole guard
      // is decoration.
      expect(found, "PtyViewport::PLAUSIBLE_MIN not found in the Rust sources").toHaveLength(1);
      expect(found[0]).toEqual({
        cols: BACKEND_SPAWN_FLOOR.cols,
        rows: BACKEND_SPAWN_FLOOR.rows,
      });
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

  describe("ui terminal controller", () => {
    const input = () => ({
      element: document.createElement("div"),
      sessionId: "session-1",
      operation: { kind: "query" } as const,
    });
    const success = (sessionId: string) => ({
      ok: true as const,
      target: {
        sessionId,
        baseY: 10,
        viewportY: 10,
        length: 34,
        cols: 80,
        rows: 24,
        type: "normal" as const,
        atBottom: true,
      },
    });

    it("returns null without a live registration", () => {
      expect(executeUiTerminalController(input())).toBeNull();
    });

    it("replaces controllers without allowing stale cleanup to clear the replacement", () => {
      const controllerA = vi.fn(() => success("a"));
      const controllerB = vi.fn(() => success("b"));
      const cleanupA = registerUiTerminalController(controllerA);
      expect(executeUiTerminalController(input())).toEqual(success("a"));

      const cleanupB = registerUiTerminalController(controllerB);
      cleanupA();
      expect(executeUiTerminalController(input())).toEqual(success("b"));
      expect(controllerA).toHaveBeenCalledTimes(1);
      expect(controllerB).toHaveBeenCalledTimes(1);

      cleanupB();
      cleanupB();
      expect(executeUiTerminalController(input())).toBeNull();
    });

    it("uses registration tokens even when the same function registers twice", () => {
      const controller = vi.fn(() => success("same"));
      const cleanupA = registerUiTerminalController(controller);
      const cleanupB = registerUiTerminalController(controller);
      cleanupA();
      expect(executeUiTerminalController(input())).toEqual(success("same"));
      cleanupB();
      expect(executeUiTerminalController(input())).toBeNull();
    });

    it("reset clears the controller and execution does not swallow errors", () => {
      registerUiTerminalController(() => {
        throw new Error("controller failed");
      });
      expect(() => executeUiTerminalController(input())).toThrow("controller failed");

      resetUiTerminalControllerForTests();
      expect(executeUiTerminalController(input())).toBeNull();
    });
  });
});
