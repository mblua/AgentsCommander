import { afterEach, describe, expect, it, vi } from "vitest";
import {
  __setTransportForTests,
  decodePtyOutputEvent,
  decodeTerminalDocumentEpoch,
  decodeTerminalOutputActivation,
  onPtyOutput,
} from "./ipc";
import { FakeTransport } from "./testing/fake-transport";
import {
  TEST_TERMINAL_DOCUMENT_EPOCH,
  terminalActivationWire,
  terminalSeedlessActivationWire,
  terminalSnapshotWire,
} from "./testing/terminal-output";
import { SESSION_A } from "./testing/session-selection";

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "terminal" }),
}));

const GENERATION = 7;
const REQUEST = {
  attachGeneration: GENERATION,
  documentEpoch: TEST_TERMINAL_DOCUMENT_EPOCH,
};

function expectActivationRejected(value: unknown): void {
  expect(() =>
    decodeTerminalOutputActivation(
      value,
      TEST_TERMINAL_DOCUMENT_EPOCH,
      GENERATION,
    ),
  ).toThrow(/Invalid terminal payload/);
}

function activationWithSnapshot(
  overrides: Readonly<Record<string, unknown>>,
): Record<string, unknown> {
  return {
    snapshot: { ...terminalSnapshotWire({ replayData: [65] }), ...overrides },
    ...REQUEST,
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("terminal output trust-boundary decoders", () => {
  it("accepts and freezes canonical normal, alternate, checkpoint, and seedless activations", () => {
    const normalWire = terminalActivationWire(REQUEST, {
      replayData: [65, 66],
      sequence: 9,
      historyIncluded: true,
      retainedHistoryRows: 2,
      includedHistoryRows: 1,
      semanticHistoryBytes: 1,
    });
    const normal = decodeTerminalOutputActivation(
      normalWire,
      TEST_TERMINAL_DOCUMENT_EPOCH,
      GENERATION,
    );
    expect(normal).toMatchObject({
      attachGeneration: GENERATION,
      documentEpoch: TEST_TERMINAL_DOCUMENT_EPOCH,
      seedlessReason: null,
      snapshot: {
        activeBuffer: "normal",
        alternateEntryMode: null,
        replayStage: "semanticHistory",
      },
    });
    expect(Object.isFrozen(normal)).toBe(true);
    expect(Object.isFrozen(normal.snapshot)).toBe(true);
    expect(Object.isFrozen(normal.snapshot?.replayData)).toBe(true);

    const alternate = decodeTerminalOutputActivation(
      terminalActivationWire(REQUEST, {
        replayData: [27, 91, 63, 49, 48, 52, 57, 104],
        activeBuffer: "alternate",
        alternateEntryMode: "mode1049",
        normalScreenIncluded: true,
      }),
      TEST_TERMINAL_DOCUMENT_EPOCH,
      GENERATION,
    );
    expect(alternate.snapshot).toMatchObject({
      activeBuffer: "alternate",
      alternateEntryMode: "mode1049",
      normalScreenIncluded: true,
    });

    const checkpoint = decodeTerminalOutputActivation(
      terminalActivationWire(REQUEST, {
        replayData: [27, 91, 63, 52, 55, 104],
        activeBuffer: "alternate",
        alternateEntryMode: "mode47",
        replayStage: "screenOnlyCheckpointUnavailable",
        normalScreenIncluded: false,
      }),
      TEST_TERMINAL_DOCUMENT_EPOCH,
      GENERATION,
    );
    expect(checkpoint.snapshot?.replayStage).toBe("screenOnlyCheckpointUnavailable");

    expect(
      decodeTerminalOutputActivation(
        terminalSeedlessActivationWire(REQUEST, "seedlessContinuationUnsafe"),
        TEST_TERMINAL_DOCUMENT_EPOCH,
        GENERATION,
      ),
    ).toEqual({
      snapshot: null,
      seedlessReason: "seedlessContinuationUnsafe",
      attachGeneration: GENERATION,
      documentEpoch: TEST_TERMINAL_DOCUMENT_EPOCH,
    });
  });

  it("keeps document epochs opaque and rejects every noncanonical positive-u64 form", () => {
    expect(decodeTerminalDocumentEpoch("18446744073709551615")).toBe(
      "18446744073709551615",
    );
    for (const hostile of [
      "",
      "0",
      "01",
      "+1",
      "-1",
      "1.0",
      " 1",
      "18446744073709551616",
      1,
      null,
    ]) {
      expect(() => decodeTerminalDocumentEpoch(hostile)).toThrow(
        /Invalid terminal payload/,
      );
    }
  });

  it("accepts canonical sequenced and unsequenced output as immutable cloned bytes", () => {
    const wire = { sessionId: SESSION_A, data: [0, 127, 255], sequence: 12 };
    const decoded = decodePtyOutputEvent(wire);
    wire.data[0] = 99;
    expect(decoded).toEqual({ sessionId: SESSION_A, data: [0, 127, 255], sequence: 12 });
    expect(Object.isFrozen(decoded)).toBe(true);
    expect(Object.isFrozen(decoded.data)).toBe(true);
    expect(decodePtyOutputEvent({ sessionId: SESSION_A, data: [] })).toEqual({
      sessionId: SESSION_A,
      data: [],
    });
  });

  it("rejects hostile output records, UUIDs, arrays, bytes, sequences, accessors, and prototypes", () => {
    const sparse: unknown[] = [];
    sparse.length = 1;
    const accessorBytes = [1];
    Object.defineProperty(accessorBytes, "0", {
      configurable: true,
      enumerable: true,
      get: () => 1,
    });
    const accessorRecord = { sessionId: SESSION_A, data: [1] };
    Object.defineProperty(accessorRecord, "sessionId", {
      configurable: true,
      enumerable: true,
      get: () => SESSION_A,
    });
    class HostileRecord {}
    const inherited = Object.assign(new HostileRecord(), {
      sessionId: SESSION_A,
      data: [1],
    });
    const extraArray = [1];
    Object.defineProperty(extraArray, "extra", { value: true, enumerable: true });

    for (const hostile of [
      null,
      [],
      { sessionId: "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA", data: [1] },
      { sessionId: "not-a-uuid", data: [1] },
      { sessionId: SESSION_A, data: "1" },
      { sessionId: SESSION_A, data: sparse },
      { sessionId: SESSION_A, data: accessorBytes },
      { sessionId: SESSION_A, data: extraArray },
      { sessionId: SESSION_A, data: [-1] },
      { sessionId: SESSION_A, data: [1.5] },
      { sessionId: SESSION_A, data: [256] },
      { sessionId: SESSION_A, data: [1], sequence: -1 },
      { sessionId: SESSION_A, data: [1], sequence: 1.5 },
      { sessionId: SESSION_A, data: [1], sequence: Number.MAX_SAFE_INTEGER + 1 },
      { sessionId: SESSION_A, data: [1], extra: true },
      accessorRecord,
      inherited,
    ]) {
      expect(() => decodePtyOutputEvent(hostile)).toThrow(/Invalid terminal payload/);
    }
  });

  it("rejects malformed activation ownership, discriminants, enums, fields, and object shapes", () => {
    const both = terminalActivationWire(REQUEST, { replayData: [] });
    both.seedlessReason = "seedlessParserUnavailable";
    const neither = { ...REQUEST };
    const accessor = terminalActivationWire(REQUEST, { replayData: [] });
    Object.defineProperty(accessor, "documentEpoch", {
      configurable: true,
      enumerable: true,
      get: () => TEST_TERMINAL_DOCUMENT_EPOCH,
    });
    class HostileActivation {}
    const inherited = Object.assign(
      new HostileActivation(),
      terminalSeedlessActivationWire(REQUEST),
    );

    for (const hostile of [
      both,
      neither,
      { ...terminalSeedlessActivationWire(REQUEST), seedlessReason: "unknown" },
      { ...terminalSeedlessActivationWire(REQUEST), attachGeneration: 0 },
      { ...terminalSeedlessActivationWire(REQUEST), attachGeneration: 1.5 },
      { ...terminalSeedlessActivationWire(REQUEST), attachGeneration: 4_294_967_296 },
      { ...terminalSeedlessActivationWire(REQUEST), attachGeneration: GENERATION + 1 },
      { ...terminalSeedlessActivationWire(REQUEST), documentEpoch: "2" },
      { ...terminalSeedlessActivationWire(REQUEST), extra: true },
      accessor,
      inherited,
    ]) {
      expectActivationRejected(hostile);
    }
  });

  it("rejects invalid bytes, grids, counters, enums, missing fields, and snapshot contradictions", () => {
    const missing = terminalSnapshotWire({ replayData: [65] });
    delete missing.rows;
    const extra = terminalSnapshotWire({ replayData: [65] });
    extra.content = "forbidden";

    for (const hostile of [
      { snapshot: missing, ...REQUEST },
      { snapshot: extra, ...REQUEST },
      activationWithSnapshot({ replayData: [256], replayBytes: 1 }),
      activationWithSnapshot({ rows: 0 }),
      activationWithSnapshot({ cols: 65_536 }),
      activationWithSnapshot({ sequence: Number.MAX_SAFE_INTEGER + 1 }),
      activationWithSnapshot({ activeBuffer: "unknown" }),
      activationWithSnapshot({ replayStage: "unknown" }),
      activationWithSnapshot({ historyTruncationReason: "unknown" }),
      activationWithSnapshot({ replayBytes: 2 }),
      activationWithSnapshot({ pendingParserBytes: 65 }),
      activationWithSnapshot({ pendingParserBytes: 2 }),
      activationWithSnapshot({ semanticHistoryBytes: 2 }),
      activationWithSnapshot({ retainedHistoryRows: 0, includedHistoryRows: 1, historyIncluded: true }),
      activationWithSnapshot({ includedHistoryRows: 1, historyIncluded: false }),
      activationWithSnapshot({ includedHistoryRows: 0, semanticHistoryBytes: 1 }),
      activationWithSnapshot({ historyTruncated: true, historyTruncationReason: "none" }),
      activationWithSnapshot({ historyBoundaryHardened: true }),
      activationWithSnapshot({ activeBuffer: "normal", alternateEntryMode: "mode47" }),
      activationWithSnapshot({ activeBuffer: "alternate" }),
      activationWithSnapshot({ activeBottomLineHasText: true, activeScreenHasText: false }),
      activationWithSnapshot({
        activeBuffer: "alternate",
        alternateEntryMode: "mode1049",
        normalScreenIncluded: false,
      }),
      activationWithSnapshot({
        replayStage: "screenOnlyHistoryDisabled",
        historyIncluded: true,
        retainedHistoryRows: 1,
        includedHistoryRows: 1,
        semanticHistoryBytes: 1,
      }),
      activationWithSnapshot({
        activeBuffer: "alternate",
        alternateEntryMode: "mode1049",
        replayStage: "screenOnlyCheckpointUnavailable",
        normalScreenIncluded: false,
      }),
    ]) {
      expectActivationRejected(hostile);
    }
  });

  it("drops malformed live payloads before callback state and diagnoses only once", async () => {
    const fake = new FakeTransport();
    const restoreTransport = __setTransportForTests(fake);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const callback = vi.fn();
    try {
      const unlisten = await onPtyOutput(callback);
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: [256] });
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: [257] });
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: [65], sequence: 1 });
      expect(callback).toHaveBeenCalledTimes(1);
      expect(callback).toHaveBeenCalledWith({
        sessionId: SESSION_A,
        data: [65],
        sequence: 1,
      });
      expect(warn).toHaveBeenCalledTimes(1);
      expect(warn).toHaveBeenCalledWith(
        "[terminal-snapshot] event=pty_output_dropped reason=malformed",
      );
      unlisten();
    } finally {
      restoreTransport();
    }
  });
});
