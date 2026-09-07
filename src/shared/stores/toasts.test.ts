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

// #1857: the aggregated blocked-menu notice is a single PINNED toast. These
// pin the two measured ways the old per-session toasts were lost: eviction past
// MAX_VISIBLE, and the dismiss animation deleting a freshly re-pushed message.
describe("pinned toasts and the dismiss/re-push race (#1857)", () => {
  it("1. survives the measured threshold: four sticky errors then a pinned info", () => {
    for (let i = 0; i < 4; i++) toastStore.error(`error-${i}`);
    toastStore.push({ message: "blocked", kind: "info", durationMs: null, pinned: true });
    // Assert on the ARRAY, never on the returned id: without `pinned`, push
    // returns a valid id for a toast that was evicted before it was ever painted.
    expect(toastStore.items.some((t) => t.message === "blocked")).toBe(true);
  });

  it("2. the threshold is exactly four: the pinned toast survives 0..3 prior errors too", () => {
    // Characterisation, NOT a falsifier: all four sub-cases pass against the
    // pre-#1857 implementation as well. It pins WHERE the measured threshold is;
    // test 1 is the falsifier. See acceptance criterion 6.
    for (const priorErrors of [0, 1, 2, 3]) {
      toastStore.clear();
      for (let i = 0; i < priorErrors; i++) toastStore.error(`error-${i}`);
      toastStore.push({ message: "blocked", kind: "info", durationMs: null, pinned: true });
      expect(toastStore.items.some((t) => t.message === "blocked")).toBe(true);
    }
  });

  it("3. a pinned toast is never the victim: a fifth error evicts an error instead", () => {
    for (let i = 0; i < 4; i++) toastStore.error(`error-${i}`);
    toastStore.push({ message: "blocked", kind: "info", durationMs: null, pinned: true });
    toastStore.error("error-4");
    expect(toastStore.items.some((t) => t.message === "blocked")).toBe(true);
    // Tier 2 took an unpinned error. That is the deliberate trade-off.
    expect(toastStore.items.some((t) => t.message === "error-1")).toBe(false);
    expect(toastStore.items).toHaveLength(4);
  });

  it("4. the cap is still honest with a pinned toast up", () => {
    toastStore.push({ message: "blocked", kind: "info", durationMs: null, pinned: true });
    for (let i = 0; i < 4; i++) toastStore.info(`info-${i}`);
    // MAX_VISIBLE is 4 and stays 4: pinning exempts a toast from the first two
    // eviction tiers, not from the cap.
    expect(toastStore.items).toHaveLength(4);
    expect(toastStore.items.some((t) => t.message === "blocked")).toBe(true);
  });

  it("5. the physical-ceiling contract: five plain toasts leave exactly four", () => {
    // `.toast-host` in src/shared/styles/toast.css declares NO `overflow` and NO
    // `max-height`, so this cap is the only thing bounding the stack height on
    // screen. Host scrolling is out of scope for this epic (and `pointer-events:
    // none` on that rule would make the scrollbar unusable anyway).
    for (let i = 0; i < 5; i++) toastStore.info(`plain-${i}`);
    expect(toastStore.items).toHaveLength(4);
  });

  it("6. a tagged re-push REVIVES a dying toast and keeps the new message", async () => {
    vi.useFakeTimers();
    try {
      toastStore.push({ message: "old text", durationMs: null, tag: "agg" });
      toastStore.dismissByTag("agg");
      expect(toastStore.items[0].exiting).toBe(true);

      await vi.advanceTimersByTimeAsync(50);
      toastStore.push({ message: "new text", durationMs: null, tag: "agg" });
      expect(toastStore.items[0].exiting).toBe(false);

      // Past TOAST_EXIT_MS counted from the ORIGINAL dismiss. Reverting the
      // revive kills the toast here, at exactly t=180.
      await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
      expect(toastStore.items).toHaveLength(1);
      expect(toastStore.items[0].message).toBe("new text");
    } finally {
      toastStore.clear();
      vi.useRealTimers();
    }
  });

  it("7. control: a normal dismiss with no re-push still removes the toast", async () => {
    vi.useFakeTimers();
    try {
      toastStore.push({ message: "old text", durationMs: null, tag: "agg" });
      toastStore.dismissByTag("agg");
      await vi.advanceTimersByTimeAsync(50 + TOAST_EXIT_MS);
      expect(toastStore.items).toHaveLength(0);
    } finally {
      toastStore.clear();
      vi.useRealTimers();
    }
  });

  it("20. the revive re-arms the auto-dismiss timer", async () => {
    vi.useFakeTimers();
    try {
      toastStore.push({ message: "first", durationMs: 1000, tag: "agg" });
      toastStore.dismissByTag("agg");
      await vi.advanceTimersByTimeAsync(50);
      toastStore.push({ message: "second", durationMs: 1000, tag: "agg" });
      expect(toastStore.items).toHaveLength(1);

      // `startToastExit` cleared the duration timer and the tag branch returns
      // without re-arming it, so without the re-arm this toast stays up forever.
      await vi.advanceTimersByTimeAsync(1000 + TOAST_EXIT_MS);
      expect(toastStore.items).toHaveLength(0);
    } finally {
      toastStore.clear();
      vi.useRealTimers();
    }
  });

  it("20b. the re-arm is a no-op for a durationMs: null toast revived the same way", async () => {
    vi.useFakeTimers();
    try {
      toastStore.push({ message: "sticky first", durationMs: null, tag: "agg" });
      toastStore.dismissByTag("agg");
      await vi.advanceTimersByTimeAsync(50);
      toastStore.push({ message: "sticky second", durationMs: null, tag: "agg" });

      await vi.advanceTimersByTimeAsync(1000 + TOAST_EXIT_MS);
      expect(toastStore.items).toHaveLength(1);
      expect(toastStore.items[0].message).toBe("sticky second");
    } finally {
      toastStore.clear();
      vi.useRealTimers();
    }
  });
});
