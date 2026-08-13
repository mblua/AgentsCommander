// @vitest-environment node
//
// #1283 - terminal-output-admission contract (plan 9.2, 14.2, 14.5).
//
// Deterministic: fake frame scheduler, fake clock, fake xterm write adapter.
// No production sleep, real animation frame, or wall-clock assertion anywhere.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  createTerminalOutputAdmission,
  parseCanonicalCounter,
  RENDER_PENDING_LIMIT_BYTES,
  type TerminalOutputAdmission,
  type TerminalOutputAdmissionOptions,
} from "./terminal-output-admission";

interface RecordedWrite {
  bytes: number[];
  callback: () => void;
}

interface Harness {
  admission: TerminalOutputAdmission;
  writes: RecordedWrite[];
  acks: { sessionId: string; generation: string; firstSequence: string; sequence: string }[];
  resyncs: { sessionId: string; generation: string }[];
  frameQueue: (() => void)[];
  clock: { now: number };
  releaseWrite(index: number): void;
  releaseNextWrite(): RecordedWrite;
  runFrames(): void;
  dataDelivery(
    overrides?: Partial<{
      sessionId: string;
      generation: string;
      firstSequence: string;
      sequence: string;
      data: number[];
    }>
  ): ReturnType<TerminalOutputAdmission["accept"]>;
  markerDelivery(generation?: string): ReturnType<TerminalOutputAdmission["accept"]>;
}

const SESSION = "11111111-1111-4111-8111-111111111111";
const GENERATION = "7";

function createHarness(
  overrides: Partial<TerminalOutputAdmissionOptions> = {}
): Harness {
  const writes: RecordedWrite[] = [];
  const acks: Harness["acks"] = [];
  const resyncs: Harness["resyncs"] = [];
  const frameQueue: (() => void)[] = [];
  const clock = { now: 1_000 };

  const admission = createTerminalOutputAdmission(SESSION, {
    write: (bytes, callback) => {
      writes.push({ bytes: Array.from(bytes), callback });
    },
    scheduleFrame: (callback) => {
      frameQueue.push(callback);
      return frameQueue.length;
    },
    cancelFrame: (handle) => {
      frameQueue[handle - 1] = () => undefined;
    },
    now: () => clock.now,
    acknowledge: (sessionId, generation, firstSequence, sequence) => {
      acks.push({ sessionId, generation, firstSequence, sequence });
    },
    resync: (sessionId, generation) => {
      resyncs.push({ sessionId, generation });
    },
    ...overrides,
  });

  return {
    admission,
    writes,
    acks,
    resyncs,
    frameQueue,
    clock,
    releaseWrite(index) {
      writes[index]?.callback();
    },
    releaseNextWrite() {
      const next = writes[writes.length - 1];
      next.callback();
      return next;
    },
    runFrames() {
      while (frameQueue.length > 0) {
        const callback = frameQueue.shift();
        callback?.();
      }
    },
    dataDelivery(overrides = {}) {
      return admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: [65],
        ...overrides,
      });
    },
    markerDelivery(generation = GENERATION) {
      return admission.accept({
        kind: "resyncRequired",
        sessionId: SESSION,
        generation,
        sequence: "0",
      });
    },
  };
}

function beginReplay(h: Harness, snapshotSequence = "5"): number {
  return h.admission.beginSnapshotReplay(3, parseCanonicalCounter(GENERATION)!, snapshotSequence);
}

describe("terminal-output-admission", () => {
  beforeEach(() => {
    // deterministic starting clock
  });

  afterEach(() => {
    // nothing retained across tests: each harness is fresh
  });

  describe("canonical counter parsing (plan 14.2)", () => {
    it("accepts canonical unsigned decimal strings only", () => {
      expect(parseCanonicalCounter("0")).toBe(0n);
      expect(parseCanonicalCounter("18446744073709551615")).toBe(18446744073709551615n);
      expect(parseCanonicalCounter("007")).toBeNull();
      expect(parseCanonicalCounter("-1")).toBeNull();
      expect(parseCanonicalCounter("1.5")).toBeNull();
      expect(parseCanonicalCounter("1e3")).toBeNull();
      expect(parseCanonicalCounter(" 1")).toBeNull();
      expect(parseCanonicalCounter("1 ")).toBeNull();
      expect(parseCanonicalCounter("")).toBeNull();
      expect(parseCanonicalCounter("abc")).toBeNull();
    });
  });

  describe("ReplayPending admission (plan 5.4.3)", () => {
    it("retains S+1 under the bound and acknowledges only after complete retention", () => {
      const h = createHarness();
      beginReplay(h, "5");

      const verdict = h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "7",
        data: [66, 67],
      });

      expect(verdict).toBe("accepted");
      expect(h.acks).toEqual([
        { sessionId: SESSION, generation: GENERATION, firstSequence: "6", sequence: "7" },
      ]);
      expect(h.writes).toHaveLength(0); // ReplayPending NEVER writes
      expect(h.admission.pendingBytes()).toBe(2);
      expect(h.admission.nextSequence()).toBe(8n);
      expect(h.admission.stateKind()).toBe("replayPending");
      expect(h.admission.hasStrongReplayGate()).toBe(true);
    });

    it("acknowledges a wholly snapshot-represented range without allocation", () => {
      const h = createHarness();
      beginReplay(h, "5");

      const verdict = h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "3",
        sequence: "5",
        data: [90, 91, 92],
      });

      expect(verdict).toBe("ackedSnapshotRepresented");
      expect(h.acks).toHaveLength(1);
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.writes).toHaveLength(0);
      expect(h.admission.nextSequence()).toBe(6n); // anchor untouched
    });

    it("seals without acknowledgement on a straddling range", () => {
      const h = createHarness();
      beginReplay(h, "5");

      const verdict = h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "4",
        sequence: "6",
        data: [65, 66, 67],
      });

      expect(verdict).toBe("recovered");
      expect(h.acks).toHaveLength(0);
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.admission.stateKind()).toBe("sealed");
      expect(h.resyncs).toEqual([{ sessionId: SESSION, generation: GENERATION }]);
      expect(h.admission.hasStrongReplayGate()).toBe(false);
    });

    it("seals without acknowledgement on a non-contiguous (gap or duplicate) range", () => {
      const h = createHarness();
      beginReplay(h, "5");
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });

      // duplicate of the retained range (firstSequence 6 != next 7)
      const duplicate = h.dataDelivery({ firstSequence: "6", sequence: "6", data: [66] });
      expect(duplicate).toBe("recovered");
      expect(h.acks).toHaveLength(1); // only the first retention acked
      expect(h.admission.stateKind()).toBe("sealed");
      expect(h.resyncs).toHaveLength(1);

      const h2 = createHarness();
      beginReplay(h2, "5");
      // gap: 8 != next 6
      const gap = h2.dataDelivery({ firstSequence: "8", sequence: "9", data: [72, 73] });
      expect(gap).toBe("recovered");
      expect(h2.acks).toHaveLength(0);
      expect(h2.admission.stateKind()).toBe("sealed");
    });

    it("rejects stale generations and foreign sessions without allocation", () => {
      const h = createHarness();
      beginReplay(h, "5");

      expect(h.dataDelivery({ generation: "8" })).toBe("rejected");
      expect(h.dataDelivery({ sessionId: "22222222-2222-4222-8222-222222222222" })).toBe(
        "rejected"
      );
      expect(h.dataDelivery({ firstSequence: "x", sequence: "6" })).toBe("recovered");
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.acks).toHaveLength(0);
      expect(h.admission.metrics().inactiveOrStaleEventsRejected).toBe(2);
    });

    it("never writes before the matching replay callback", () => {
      const h = createHarness();
      beginReplay(h, "5");
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });

      expect(h.writes).toHaveLength(0);
      h.admission.completeSnapshotReplay(999); // wrong token: inert
      expect(h.writes).toHaveLength(0);
      expect(h.admission.stateKind()).toBe("replayPending");
      expect(h.admission.hasStrongReplayGate()).toBe(true);
    });
  });

  describe("snapshot renderer (plan 7 activation-payload-only)", () => {
    it("writes the exact activation payload and drains retained S+1 exactly once", () => {
      const h = createHarness();
      const replayToken = beginReplay(h, "5");
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });

      h.admission.renderSnapshot([83, 78, 65, 80], "5");
      expect(h.writes).toHaveLength(1);
      expect(h.writes[0].bytes).toEqual([83, 78, 65, 80]);

      h.releaseNextWrite();
      expect(h.admission.stateKind()).toBe("live");
      expect(h.admission.hasStrongReplayGate()).toBe(false);
      expect(h.admission.hasStrongWriteGate()).toBe(false);

      // Retained S+1 drains exactly once after the replay callback.
      h.runFrames();
      expect(h.writes).toHaveLength(2);
      expect(h.writes[1].bytes).toEqual([66]);
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.admission.writeInFlightBytes()).toBe(1);
      expect(h.admission.hasStrongWriteGate()).toBe(true);

      h.releaseNextWrite();
      expect(h.admission.writeInFlightBytes()).toBe(0);
      expect(h.admission.hasStrongWriteGate()).toBe(false);
    });

    it("rejects a snapshot sequence that does not equal the anchor S", () => {
      const h = createHarness();
      beginReplay(h, "5");

      h.admission.renderSnapshot([83], "6");
      expect(h.writes).toHaveLength(0);
      expect(h.admission.stateKind()).toBe("sealed");
      expect(h.resyncs).toHaveLength(1);
    });

    it("renders through the injected adapter with a weak-only replay callback capture", () => {
      const h = createHarness();
      const replayToken = beginReplay(h, "5");
      h.admission.renderSnapshot([65], "5");

      // The callback is the only thing the adapter holds; after disposal it
      // must be a no-op even when forced.
      h.admission.dispose();
      expect(h.admission.hasStrongReplayGate()).toBe(false);
      expect(h.admission.metrics().retiredWriteCallbacksIgnoredAfterDisposal).toBe(0);
      h.releaseNextWrite();
      expect(h.admission.stateKind()).toBe("idle");
      expect(h.admission.metrics().retiredWriteCallbacksIgnoredAfterDisposal).toBe(1);
      // no mutation happened
      expect(h.admission.nextSequence()).toBeNull();
      void replayToken;
    });
  });

  describe("checked byte bound (plan 5.4 / 14.5.2)", () => {
    it("retains and acknowledges an exact-limit replay queue", () => {
      const h = createHarness();
      beginReplay(h, "5");
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "6",
        data: new Array<number>(RENDER_PENDING_LIMIT_BYTES).fill(66),
      });

      expect(h.admission.stateKind()).toBe("replayPending");
      expect(h.admission.pendingBytes()).toBe(RENDER_PENDING_LIMIT_BYTES);
      expect(h.acks).toHaveLength(1);
    });

    it("seals without acknowledgement at limit plus one byte", () => {
      const h = createHarness();
      beginReplay(h, "5");
      const verdict = h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "6",
        data: new Array<number>(RENDER_PENDING_LIMIT_BYTES + 1).fill(66),
      });

      expect(verdict).toBe("recovered");
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.acks).toHaveLength(0);
      expect(h.admission.stateKind()).toBe("sealed");
      expect(h.resyncs).toHaveLength(1);
    });

    it("fails an exact-limit held write before allocation on one more byte", () => {
      const h = createHarness();
      beginReplay(h, "5");
      h.admission.renderSnapshot([83], "5");
      h.releaseNextWrite();
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "6",
        data: new Array<number>(RENDER_PENDING_LIMIT_BYTES).fill(67),
      });
      expect(h.admission.stateKind()).toBe("live");
      expect(h.admission.pendingBytes()).toBe(RENDER_PENDING_LIMIT_BYTES);
      expect(h.acks).toHaveLength(1);
      h.runFrames();
      expect(h.writes).toHaveLength(2);
      expect(h.admission.hasStrongWriteGate()).toBe(true);
      expect(h.admission.writeInFlightBytes()).toBe(RENDER_PENDING_LIMIT_BYTES);

      // One byte over the checked write_in_flight + pending + incoming sum.
      const beforeAcks = h.acks.length;
      const verdict = h.dataDelivery({ firstSequence: "7", sequence: "7", data: [68] });
      expect(verdict).toBe("recovered");
      expect(h.acks).toHaveLength(beforeAcks); // no normal acknowledgement
      expect(h.admission.stateKind()).toBe("sealed");
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.admission.writeInFlightBytes()).toBe(0); // synchronously released

      // The held write callback is inert after the seal.
      h.releaseNextWrite();
      expect(h.admission.metrics().retiredWriteCallbacksIgnoredAfterDisposal).toBe(1);
      expect(h.admission.hasStrongWriteGate()).toBe(false);
    });
  });

  describe("live admission and one-frame writer (plan 5.5.5)", () => {
    it("groups accepted data into one Uint8Array per frame, preserving order", () => {
      const h = createHarness();
      beginReplay(h, "0");
      h.admission.renderSnapshot([83], "0");
      h.releaseNextWrite();

      h.dataDelivery({ firstSequence: "1", sequence: "1", data: [65] });
      h.dataDelivery({ firstSequence: "2", sequence: "3", data: [66, 67] });
      h.dataDelivery({ firstSequence: "4", sequence: "4", data: [68] });

      expect(h.writes).toHaveLength(1); // snapshot only
      h.runFrames();
      expect(h.writes).toHaveLength(2);
      expect(h.writes[1].bytes).toEqual([65, 66, 67, 68]);
      expect(h.admission.metrics().bytesWritten).toBe(4);

      // The next write starts only from the previous write callback.
      h.dataDelivery({ firstSequence: "5", sequence: "5", data: [69] });
      expect(h.writes).toHaveLength(2);
      h.releaseNextWrite();
      h.runFrames();
      expect(h.writes).toHaveLength(3);
      expect(h.writes[2].bytes).toEqual([69]);
    });

    it("keeps exactly one frame callback and one write in flight", () => {
      const h = createHarness();
      beginReplay(h, "0");
      h.admission.renderSnapshot([83], "0");
      h.releaseNextWrite();

      h.dataDelivery({ firstSequence: "1", sequence: "1", data: [65] });
      h.dataDelivery({ firstSequence: "2", sequence: "2", data: [66] });
      expect(h.frameQueue.length).toBe(1); // one animation-frame callback at most

      h.runFrames();
      expect(h.writes).toHaveLength(2);
      expect(h.admission.hasStrongWriteGate()).toBe(true);

      h.dataDelivery({ firstSequence: "3", sequence: "3", data: [67] });
      h.runFrames(); // frame runs but the in-flight gate blocks a second write
      expect(h.writes).toHaveLength(2);

      h.releaseNextWrite();
      h.runFrames();
      expect(h.writes).toHaveLength(3);
    });
  });

  describe("delayed normal settlement and strong-owner seams (plan 14.5.10)", () => {
    it("owns one strong replay gate, then clears it on normal settlement", () => {
      const h = createHarness();
      beginReplay(h, "5");
      expect(h.admission.hasStrongReplayGate()).toBe(true);

      h.admission.renderSnapshot([83], "5");
      expect(h.admission.hasStrongReplayGate()).toBe(true);

      h.admission.completeSnapshotReplay(0); // wrong token: no transition
      expect(h.admission.stateKind()).toBe("replayPending");
      expect(h.admission.hasStrongReplayGate()).toBe(true);

      h.releaseNextWrite();
      expect(h.admission.stateKind()).toBe("live");
      expect(h.admission.hasStrongReplayGate()).toBe(false);
    });

    it("owns one strong write gate only while a live write is in flight", () => {
      const h = createHarness();
      beginReplay(h, "0");
      h.admission.renderSnapshot([83], "0");
      h.releaseNextWrite();
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: [84],
      });
      h.runFrames();

      expect(h.admission.hasStrongWriteGate()).toBe(true);
      h.admission.completeWrite(0); // wrong token: no release
      expect(h.admission.hasStrongWriteGate()).toBe(true);

      h.releaseNextWrite();
      expect(h.admission.hasStrongWriteGate()).toBe(false);
      expect(h.admission.writeInFlightBytes()).toBe(0);
    });
  });

  describe("seal, disposal, and recovery (plan 5.4.6 / 5.4.8)", () => {
    it("releases every counter and raw byte synchronously without waiting for a held callback", () => {
      const h = createHarness();
      beginReplay(h, "0");
      h.admission.renderSnapshot([83], "0");
      h.releaseNextWrite();
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: [84],
      });
      h.runFrames();
      expect(h.admission.hasStrongWriteGate()).toBe(true);
      expect(h.admission.writeInFlightBytes()).toBe(1);

      h.admission.sealGeneration();
      expect(h.admission.stateKind()).toBe("sealed");
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.admission.writeInFlightBytes()).toBe(0);
      expect(h.admission.hasStrongWriteGate()).toBe(false);
      expect(h.resyncs).toHaveLength(0); // silent seal

      h.releaseNextWrite();
      expect(h.admission.metrics().retiredWriteCallbacksIgnoredAfterDisposal).toBe(1);
      expect(h.admission.stateKind()).toBe("sealed");
    });

    it("issues exactly one resync request per sealed recovery", () => {
      const h = createHarness();
      beginReplay(h, "5");
      h.dataDelivery({ firstSequence: "9", sequence: "9", data: [72] }); // gap
      expect(h.resyncs).toHaveLength(1);
      h.dataDelivery({ firstSequence: "9", sequence: "9", data: [73] }); // sealed: rejected
      expect(h.admission.stateKind()).toBe("sealed");
      expect(h.admission.metrics().resyncCount).toBe(1);
    });

    it("treats a resyncRequired marker as one sealed recovery for the matching generation", () => {
      const h = createHarness();
      beginReplay(h, "5");
      expect(h.markerDelivery()).toBe("recovered");
      expect(h.admission.stateKind()).toBe("sealed");
      expect(h.resyncs).toHaveLength(1);
      expect(h.admission.metrics().resyncCount).toBe(1);
      expect(h.admission.metrics().inactiveOrStaleEventsRejected).toBe(0);

      // A stale-generation marker is a plain rejection.
      const h2 = createHarness();
      beginReplay(h2, "5");
      expect(h2.markerDelivery("999")).toBe("rejected");
      expect(h2.admission.stateKind()).toBe("replayPending");
    });

    it("resets to idle without a recovery request", () => {
      const h = createHarness();
      beginReplay(h, "5");
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      h.admission.reset();
      expect(h.admission.stateKind()).toBe("idle");
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.resyncs).toHaveLength(0);
      expect(h.admission.hasStrongReplayGate()).toBe(false);
    });

    it("a new beginSnapshotReplay seals the previous unsealed state first", () => {
      const h = createHarness();
      beginReplay(h, "5");
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      const newToken = h.admission.beginSnapshotReplay(4, 9n, "10");
      expect(h.admission.stateKind()).toBe("replayPending");
      expect(h.admission.pendingBytes()).toBe(0);
      expect(h.admission.snapshotSequence()).toBe(10n);
      expect(h.admission.nextSequence()).toBe(11n);
      expect(h.admission.hasStrongReplayGate()).toBe(true);
      expect(h.resyncs).toHaveLength(0); // silent replacement
      void newToken;
    });
  });

  describe("stale events and metrics (plan 5.6.1)", () => {
    it("rejects deliveries for idle and sealed states without allocation", () => {
      const h = createHarness();
      expect(h.dataDelivery()).toBe("rejected");
      expect(h.admission.metrics().inactiveOrStaleEventsRejected).toBe(1);

      beginReplay(h, "5");
      h.admission.requestResync();
      expect(h.dataDelivery()).toBe("rejected");
      expect(h.admission.metrics().inactiveOrStaleEventsRejected).toBe(2);
      expect(h.admission.pendingBytes()).toBe(0);
    });

    it("tracks gauges and high-water marks", () => {
      const h = createHarness();
      beginReplay(h, "0");
      h.admission.renderSnapshot([83], "0");
      h.releaseNextWrite();
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: [65, 66],
      });
      h.admission.accept({
        kind: "data",
        sessionId: SESSION,
        generation: GENERATION,
        firstSequence: "2",
        sequence: "2",
        data: [67],
      });
      const metrics = h.admission.metrics();
      expect(metrics.outputEventsReceived).toBe(2);
      expect(metrics.bytesAccepted).toBe(3);
      expect(metrics.livePendingBytes).toBe(3);
      expect(metrics.pendingHighWaterBytes).toBe(3);
      expect(metrics.combinedAdmissionHighWaterBytes).toBe(3);
    });
  });
});
