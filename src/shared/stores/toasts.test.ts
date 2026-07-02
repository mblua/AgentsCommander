import { afterEach, describe, expect, it, vi } from "vitest";
import { toastStore, TOAST_EXIT_MS } from "./toasts";

// The store is a module singleton, so every test must reset it. vi.useRealTimers()
// is defensive: the timed cases install fake timers and a throw inside that window
// could otherwise leak frozen timers into the next test.
afterEach(() => {
  toastStore.clear();
  vi.useRealTimers();
});

describe("toastStore (#574)", () => {
  it("push returns an id, items contains the toast, and kind defaults to info", () => {
    const id = toastStore.push({ message: "hello" });
    expect(typeof id).toBe("number");
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0]).toMatchObject({ id, kind: "info", message: "hello" });
  });

  it("error toasts are sticky (no auto-dismiss past the non-error default)", async () => {
    vi.useFakeTimers();
    try {
      toastStore.error("boom");
      expect(toastStore.items).toHaveLength(1);
      // Well past the 4000ms info/success default; the error must remain.
      await vi.advanceTimersByTimeAsync(10_000);
      expect(toastStore.items).toHaveLength(1);
      expect(toastStore.items[0].kind).toBe("error");
    } finally {
      vi.useRealTimers();
    }
  });

  it("info and success auto-dismiss after 4000ms", async () => {
    vi.useFakeTimers();
    try {
      toastStore.info("note");
      toastStore.success("done");
      expect(toastStore.items).toHaveLength(2);
      await vi.advanceTimersByTimeAsync(4000);
      expect(toastStore.items).toHaveLength(2);
      expect(toastStore.items.every((t) => t.exiting)).toBe(true);
      await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
      expect(toastStore.items).toHaveLength(0);
    } finally {
      toastStore.clear();
      vi.useRealTimers();
    }
  });

  it("honors a durationMs override in both directions", async () => {
    vi.useFakeTimers();
    try {
      // An error opted into auto-dismiss at 1000ms.
      toastStore.push({ kind: "error", durationMs: 1000, message: "transient error" });
      // An info opted into sticky.
      toastStore.push({ kind: "info", durationMs: null, message: "sticky info" });
      expect(toastStore.items).toHaveLength(2);

      await vi.advanceTimersByTimeAsync(1000);
      // The error auto-dismissed into its exit phase; the sticky info remains.
      expect(toastStore.items).toHaveLength(2);
      expect(toastStore.items.find((t) => t.message === "transient error")?.exiting).toBe(true);
      await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
      // The error finished fading out; the sticky info remains.
      expect(toastStore.items).toHaveLength(1);
      expect(toastStore.items[0].message).toBe("sticky info");

      await vi.advanceTimersByTimeAsync(10_000);
      // The sticky info never auto-dismisses.
      expect(toastStore.items).toHaveLength(1);
      expect(toastStore.items[0].message).toBe("sticky info");
    } finally {
      toastStore.clear();
      vi.useRealTimers();
    }
  });

  it("dismiss(id) fades and removes only that toast", async () => {
    vi.useFakeTimers();
    try {
      const a = toastStore.error("a");
      const b = toastStore.error("b");
      toastStore.dismiss(a);
      toastStore.dismiss(a);
      expect(toastStore.items).toHaveLength(2);
      expect(toastStore.items.find((t) => t.id === a)?.exiting).toBe(true);
      expect(toastStore.items.find((t) => t.id === b)?.exiting).toBe(false);

      await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
      expect(toastStore.items).toHaveLength(1);
      expect(toastStore.items[0].id).toBe(b);
    } finally {
      toastStore.clear();
      vi.useRealTimers();
    }
  });

  it("clear() empties items and cancels pending timers", async () => {
    vi.useFakeTimers();
    try {
      toastStore.info("note");
      expect(toastStore.items).toHaveLength(1);
      toastStore.dismiss(toastStore.items[0].id);
      toastStore.clear();
      expect(toastStore.items).toHaveLength(0);
      // The cancelled timer must not fire (no throw, items stays empty).
      await vi.advanceTimersByTimeAsync(4000 + TOAST_EXIT_MS);
      expect(toastStore.items).toHaveLength(0);
    } finally {
      toastStore.clear();
      vi.useRealTimers();
    }
  });

  describe("kind-aware eviction (§15.3)", () => {
    it("(a) same-kind FIFO: the oldest is evicted and its timer is cancelled", async () => {
      vi.useFakeTimers();
      try {
        for (let i = 0; i < 5; i++) toastStore.info(`info-${i}`);
        // MAX_VISIBLE = 4; the oldest (info-0) is evicted FIFO.
        expect(toastStore.items).toHaveLength(4);
        expect(toastStore.items.map((t) => t.message)).toEqual([
          "info-1",
          "info-2",
          "info-3",
          "info-4",
        ]);

        // If info-0's timer had survived eviction, advancing 4000ms would fire a
        // 5th (no-op) dismiss; exactly 4 proves the evicted timer was cancelled.
        const dismissSpy = vi.spyOn(toastStore, "dismiss");
        await vi.advanceTimersByTimeAsync(4000);
        expect(dismissSpy).toHaveBeenCalledTimes(4);
        expect(toastStore.items).toHaveLength(4);
        expect(toastStore.items.every((t) => t.exiting)).toBe(true);
        await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
        expect(toastStore.items).toHaveLength(0);
        dismissSpy.mockRestore();
      } finally {
        toastStore.clear();
        vi.useRealTimers();
      }
    });

    it("(b) sticky errors are protected: a transient info is the victim", () => {
      for (let i = 0; i < 4; i++) toastStore.error(`error-${i}`);
      toastStore.info("transient");
      // The info (the only non-error) is evicted; all 4 errors survive.
      expect(toastStore.items).toHaveLength(4);
      expect(toastStore.items.every((t) => t.kind === "error")).toBe(true);
      expect(toastStore.items.map((t) => t.message)).toEqual([
        "error-0",
        "error-1",
        "error-2",
        "error-3",
      ]);
    });

    it("(c) all-errors fallback: the oldest error is evicted to keep the cap honest", () => {
      for (let i = 0; i < 5; i++) toastStore.error(`error-${i}`);
      // No non-error victim exists, so the oldest overall (error-0) is evicted.
      expect(toastStore.items).toHaveLength(4);
      expect(toastStore.items.map((t) => t.message)).toEqual([
        "error-1",
        "error-2",
        "error-3",
        "error-4",
      ]);
    });
  });
});
