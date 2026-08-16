/**
 * #1283 - terminal output admission: sequence-aware `ReplayPending` and `Live`
 * admission for one session's current generation (plan 5.1 / 5.4 / 5.5 / 14.2).
 *
 * Owns: data-event sequence checks, `ReplayPending` snapshot filtering, one
 * bounded pending-byte queue shared by replay and live admission, a checked
 * `write_in_flight_bytes + pending_bytes + incoming_bytes <= 131,072`
 * invariant, one animation-frame callback, one Xterm write in flight, resync
 * coalescing, callback-gate ownership, and admission metrics.
 *
 * Gate ownership contract: the current admission state is the gate's sole
 * strong private owner - an unsealed `ReplayPending` owns exactly one
 * `replay_gate` from activation commit until its matching replay callback or
 * seal/disposal, and an unsealed `Live` state owns exactly one `write_gate`
 * only from a `Terminal.write` handoff until its matching write callback or
 * seal/disposal. Callback closures capture only primitive identity plus
 * `WeakRef(gate)`; a matching normal callback validates the live gate and the
 * current token, performs its permitted transition or counter release, then
 * marks the gate inert and clears that one strong owner. A seal or disposal
 * never waits for a callback: it first marks any gate inert and clears its
 * strong owner, then synchronously releases every pending/in-flight counter
 * and raw-byte reference, leaving a later callback inert.
 *
 * This deep module never imports TerminalView, a sidebar module,
 * `src/shared/ipc.ts`, or `@tauri-apps/api`; all effects (xterm write, frame
 * scheduling, acknowledgement, recovery) arrive through injected adapters.
 */
import type { TerminalOutputDelivery } from "../../shared/types";

/** Plan 5.3: the frontend never holds more admission bytes than this. */
export const RENDER_PENDING_LIMIT_BYTES = 131_072;

/** Plan 5.3: at most one normal live write per animation frame. */
export const UI_BATCH_INTERVAL_MS = 16;

/** Plan 5.3: frontend telemetry cadence and generation health-deadline. */
export const TELEMETRY_INTERVAL_MS = 5_000;

/** Largest canonical u64 wire value (checked arithmetic, plan 14.2). */
const MAX_WIRE_COUNTER = 0xffffffffffffffffn;

export type CanonicalCounter = bigint;

/**
 * Parses a canonical unsigned decimal string exactly once at the admission
 * boundary (plan 14.2): no signs, whitespace, fractions, exponent notation, or
 * non-canonical leading zeroes. Returns null for anything else.
 */
export function parseCanonicalCounter(value: string): CanonicalCounter | null {
  if (!/^(0|[1-9][0-9]*)$/.test(value)) {
    return null;
  }
  try {
    return BigInt(value);
  } catch {
    return null;
  }
}

/** The private callback gate: primitive identity + inert state only, never raw
 *  bytes or a strong entry/resource reference (plan 5.1). */
export interface CallbackGate {
  readonly entryId: string;
  readonly epoch: number;
  readonly generation: CanonicalCounter;
  readonly token: number;
  inert: boolean;
}

interface QueuedBatch {
  bytes: number[];
  firstSequence: CanonicalCounter;
  sequence: CanonicalCounter;
}

export type AdmissionStateKind = "idle" | "replayPending" | "live" | "sealed";

type AdmissionState =
  | { kind: "idle" }
  | {
      kind: "replayPending";
      epoch: number;
      generation: CanonicalCounter;
      snapshotSequence: CanonicalCounter;
      nextSequence: CanonicalCounter;
      replayToken: number;
      replayGate: CallbackGate | null;
      queue: QueuedBatch[];
      pendingBytes: number;
      snapshotWriteStartedAt: number | null;
    }
  | {
      kind: "live";
      epoch: number;
      generation: CanonicalCounter;
      snapshotSequence: CanonicalCounter;
      nextSequence: CanonicalCounter;
      queue: QueuedBatch[];
      pendingBytes: number;
      writeGate: CallbackGate | null;
      writeInFlightBytes: number;
      snapshotWriteStartedAt: number | null;
    }
  | { kind: "sealed"; generation: CanonicalCounter };

/** Plan 5.6.1 frontend admission counters and gauges. */
export interface AdmissionMetrics {
  outputEventsReceived: number;
  inactiveOrStaleEventsRejected: number;
  bytesAccepted: number;
  bytesWritten: number;
  replayPendingBytes: number;
  livePendingBytes: number;
  writeInFlightBytes: number;
  combinedAdmissionHighWaterBytes: number;
  pendingHighWaterBytes: number;
  resyncCount: number;
  replayPendingLivenessRecoveries: number;
  snapshotReplayDurationMs: number;
  retiredWriteCallbacksIgnoredAfterDisposal: number;
  maxAnimationFrameLagMs: number;
}

export interface TerminalOutputAdmissionOptions {
  /** Xterm write handoff (snapshot and live batches). The callback fires when
   *  xterm finished writing. */
  write: (bytes: Uint8Array, callback: () => void) => void;
  /** requestAnimationFrame adapter; exactly one frame callback may be pending. */
  scheduleFrame: (callback: () => void) => number;
  cancelFrame: (handle: number) => void;
  /** Monotonic clock (performance.now-compatible). */
  now: () => number;
  /** Normal acknowledgement: called ONLY for exact eligible retained or
   *  snapshot-represented deliveries, with the canonical string values. */
  acknowledge: (
    sessionId: string,
    generation: string,
    firstSequence: string,
    sequence: string
  ) => void;
  /** Recovery request: the admission state has been sealed and the caller
   *  starts exactly one recovery lane (deactivate + replacement activation). */
  resync: (sessionId: string, generation: string) => void;
}

export type AcceptVerdict =
  | "accepted"
  | "ackedSnapshotRepresented"
  | "rejected"
  | "recovered";

export interface TerminalOutputAdmission {
  /** Installs a fresh ReplayPending generation. Seals any previous unsealed
   *  state first (inert gates, cleared counters/raw bytes). Returns the
   *  non-reusable replay token. `snapshotSequence` is validated as a canonical
   *  counter (throws on malformed input: fail-closed invariant error). */
  beginSnapshotReplay(
    epoch: number,
    generation: CanonicalCounter,
    snapshotSequence: string
  ): number;
  /** Starts the snapshot write through the injected xterm adapter, using the
   *  exact activation payload. Validates that `sequence` is canonical and
   *  equals the committed snapshot anchor S; a mismatch seals the epoch and
   *  enters one recovery lane (plan 5.4 synchronous replay failure). The
   *  replay callback captures only primitive identity plus WeakRef(replayGate). */
  renderSnapshot(data: number[], sequence: string): void;
  /** The replay callback entry. Validates the live gate + token; on success
   *  transitions to Live(writeGate = null), inerts/clears the replay gate, and
   *  schedules at most one frame to drain the retained queue. */
  completeSnapshotReplay(replayToken: number): void;
  /** Sequence-aware admission for one `pty_output` delivery. */
  accept(delivery: TerminalOutputDelivery): AcceptVerdict;
  /** Live write callback entry. Validates the live gate + token; clears the
   *  exact in-flight counter, inerts/clears the write gate, then permits the
   *  next write. */
  completeWrite(writeToken: number): void;
  /** Silent seal: marks any gate inert and clears its strong owner, cancels
   *  the frame, zeroes counters and raw-byte ownership. No recovery request. */
  sealGeneration(): void;
  /** Seal + exactly one recovery request through the injected `resync` hook. */
  requestResync(): void;
  /** Silent teardown to idle (disposal path; no recovery request). */
  dispose(): void;
  /** Silent reset to idle (no recovery request). */
  reset(): void;
  /** Guard-level rejections (stale/inactive/detached/generation mismatch) are
   *  counted here without creating admission state (plan 5.6.1). */
  noteRejected(): void;
  /** Records a replay-pending liveness recovery (plan 5.6.1: health poll
   *  observed resyncRequired while a ReplayPending callback was held). */
  noteLivenessRecovery(): void;

  // ── read-only state / test seams ─────────────────────────────────────────
  stateKind(): AdmissionStateKind;
  generation(): CanonicalCounter | null;
  snapshotSequence(): CanonicalCounter | null;
  nextSequence(): CanonicalCounter | null;
  pendingBytes(): number;
  writeInFlightBytes(): number;
  isReplayPending(): boolean;
  hasStrongReplayGate(): boolean;
  hasStrongWriteGate(): boolean;
  metrics(): AdmissionMetrics;
}

let tokenCounter = 0;

export function createTerminalOutputAdmission(
  sessionId: string,
  options: TerminalOutputAdmissionOptions
): TerminalOutputAdmission {
  let state: AdmissionState = { kind: "idle" };
  let frameHandle: number | null = null;
  let frameScheduledAt = 0;

  // ── counters / gauges ────────────────────────────────────────────────────
  let outputEventsReceived = 0;
  let inactiveOrStaleEventsRejected = 0;
  let bytesAccepted = 0;
  let bytesWritten = 0;
  let combinedHighWater = 0;
  let pendingHighWater = 0;
  let resyncCount = 0;
  let replayPendingLivenessRecoveries = 0;
  let snapshotReplayDurationMs = 0;
  let retiredWriteCallbacksIgnoredAfterDisposal = 0;
  let maxAnimationFrameLagMs = 0;

  const trackHighWater = (): void => {
    const pending = state.kind === "live" || state.kind === "replayPending" ? state.pendingBytes : 0;
    const inFlight = state.kind === "live" ? state.writeInFlightBytes : 0;
    const combined = pending + inFlight;
    if (combined > combinedHighWater) combinedHighWater = combined;
    if (pending > pendingHighWater) pendingHighWater = pending;
  };

  const noteRetiredCallback = (): void => {
    retiredWriteCallbacksIgnoredAfterDisposal += 1;
  };

  const cancelFrame = (): void => {
    if (frameHandle !== null) {
      options.cancelFrame(frameHandle);
      frameHandle = null;
    }
  };

  /** Marks any gate in the CURRENT state inert and clears its strong owner. */
  const inertCurrentGate = (): void => {
    if (state.kind === "replayPending") {
      if (state.replayGate !== null) {
        state.replayGate.inert = true;
        state.replayGate = null;
      }
    } else if (state.kind === "live" && state.writeGate !== null) {
      state.writeGate.inert = true;
      state.writeGate = null;
    }
  };

  const sealTo = (next: AdmissionState): void => {
    inertCurrentGate();
    cancelFrame();
    if (state.kind === "replayPending" || state.kind === "live") {
      state.queue = [];
      state.pendingBytes = 0;
      if (state.kind === "live") {
        state.writeInFlightBytes = 0;
      }
    }
    state = next;
  };

  const scheduleDrain = (): void => {
    if (frameHandle !== null) {
      return; // one animation-frame callback at most
    }
    if (state.kind !== "live" || state.writeGate !== null || state.queue.length === 0) {
      return;
    }
    frameScheduledAt = options.now();
    frameHandle = options.scheduleFrame(() => {
      frameHandle = null;
      drainFrame();
    });
  };

  const drainFrame = (): void => {
    if (state.kind !== "live" || state.writeGate !== null || state.queue.length === 0) {
      return;
    }
    const lag = options.now() - frameScheduledAt;
    if (lag > maxAnimationFrameLagMs) maxAnimationFrameLagMs = lag;

    // Group every queued batch into ONE Uint8Array per frame, preserving order.
    const batches = state.queue;
    state.queue = [];
    const total = state.pendingBytes;
    state.pendingBytes = 0;
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const batch of batches) {
      bytes.set(batch.bytes, offset);
      offset += batch.bytes.length;
    }

    state.writeInFlightBytes = total;
    bytesWritten += total;
    const token = ++tokenCounter;
    const epoch = state.epoch;
    const generation = state.generation;
    const gate: CallbackGate = {
      entryId: sessionId,
      epoch,
      generation,
      token,
      inert: false,
    };
    state.writeGate = gate;
    trackHighWater();

    const gateRef = new WeakRef<CallbackGate>(gate);
    options.write(bytes, () => {
      const live = gateRef.deref();
      if (!live || live.inert) {
        noteRetiredCallback();
        return;
      }
      if (
        state.kind !== "live" ||
        state.writeGate !== live ||
        state.writeGate.token !== token ||
        state.generation !== live.generation
      ) {
        noteRetiredCallback();
        return;
      }
      // Matching normal settlement: clear in-flight, inert gate, clear owner.
      state.writeInFlightBytes = 0;
      live.inert = true;
      state.writeGate = null;
      scheduleDrain();
    });
  };

  const requestResyncLane = (): void => {
    if (state.kind === "replayPending") {
      replayPendingLivenessRecoveries += 1;
    }
    resyncCount += 1;
    if (state.kind === "sealed") {
      options.resync(sessionId, state.generation.toString());
    }
  };

  const admission: TerminalOutputAdmission = {
    beginSnapshotReplay(epoch, generation, snapshotSequence) {
      const s = parseCanonicalCounter(snapshotSequence);
      if (s === null) {
        throw new Error(
          `[admission] ${sessionId}: invalid canonical snapshot sequence '${snapshotSequence}'`
        );
      }
      sealTo({ kind: "idle" });
      const token = ++tokenCounter;
      const gate: CallbackGate = {
        entryId: sessionId,
        epoch,
        generation,
        token,
        inert: false,
      };
      state = {
        kind: "replayPending",
        epoch,
        generation,
        snapshotSequence: s,
        nextSequence: s + 1n,
        replayToken: token,
        replayGate: gate,
        queue: [],
        pendingBytes: 0,
        snapshotWriteStartedAt: null,
      };
      return token;
    },

    renderSnapshot(data, sequence) {
      if (state.kind !== "replayPending") {
        noteRetiredCallback();
        return;
      }
      const s = parseCanonicalCounter(sequence);
      if (s === null || s !== state.snapshotSequence) {
        // Synchronous replay failure: seal the epoch, clear the strong owner,
        // then enter the one recovery lane (plan 5.4 item 4).
        sealTo({ kind: "sealed", generation: state.generation });
        requestResyncLane();
        return;
      }
      const token = state.replayToken;
      const gate = state.replayGate;
      if (gate === null) {
        noteRetiredCallback();
        return;
      }
      const gateRef = new WeakRef<CallbackGate>(gate);
      state.snapshotWriteStartedAt = options.now();
      options.write(new Uint8Array(data), () => {
        const gate = gateRef.deref();
        if (!gate || gate.inert) {
          noteRetiredCallback();
          return;
        }
        admission.completeSnapshotReplay(token);
      });
    },

    completeSnapshotReplay(replayToken) {
      if (state.kind !== "replayPending") {
        noteRetiredCallback();
        return;
      }
      const gate = state.replayGate;
      if (gate === null || gate.inert || gate.token !== replayToken) {
        noteRetiredCallback();
        return;
      }
      const startedAt = state.snapshotWriteStartedAt;
      if (startedAt !== null) {
        snapshotReplayDurationMs = options.now() - startedAt;
      }
      const {
        epoch,
        generation,
        snapshotSequence,
        nextSequence,
        queue,
        pendingBytes,
      } = state;
      gate.inert = true;
      state = {
        kind: "live",
        epoch,
        generation,
        snapshotSequence,
        nextSequence,
        queue,
        pendingBytes,
        writeGate: null,
        writeInFlightBytes: 0,
        snapshotWriteStartedAt: null,
      };
      scheduleDrain();
    },

    accept(delivery) {
      outputEventsReceived += 1;
      if (delivery.kind !== "data") {
        // resyncRequired marker for the current unsealed generation.
        if (state.kind === "replayPending" || state.kind === "live") {
          const markerGeneration = parseCanonicalCounter(delivery.generation);
          if (markerGeneration !== null && markerGeneration === state.generation) {
            sealTo({ kind: "sealed", generation: state.generation });
            requestResyncLane();
            return "recovered";
          }
        }
        inactiveOrStaleEventsRejected += 1;
        return "rejected";
      }

      if (state.kind !== "replayPending" && state.kind !== "live") {
        inactiveOrStaleEventsRejected += 1;
        return "rejected";
      }
      if (delivery.sessionId !== sessionId) {
        inactiveOrStaleEventsRejected += 1;
        return "rejected";
      }
      const generation = parseCanonicalCounter(delivery.generation);
      if (generation === null || generation !== state.generation) {
        inactiveOrStaleEventsRejected += 1;
        return "rejected";
      }
      const first = parseCanonicalCounter(delivery.firstSequence);
      const last = parseCanonicalCounter(delivery.sequence);
      if (first === null || last === null || first > last) {
        // Malformed or inverted range: no ack, no retention, one recovery.
        sealTo({ kind: "sealed", generation: state.generation });
        requestResyncLane();
        return "recovered";
      }

      const s = state.snapshotSequence;
      if (last <= s) {
        // Wholly snapshot-represented: acknowledged WITHOUT allocation because
        // the activation snapshot already contains this exact range.
        options.acknowledge(
          sessionId,
          delivery.generation,
          delivery.firstSequence,
          delivery.sequence
        );
        return "ackedSnapshotRepresented";
      }

      if (first <= s) {
        // Straddles S: seal/recover, no acknowledgement, no partial retention.
        sealTo({ kind: "sealed", generation: state.generation });
        requestResyncLane();
        return "recovered";
      }

      if (first !== state.nextSequence) {
        // Duplicate or non-contiguous post-snapshot range.
        sealTo({ kind: "sealed", generation: state.generation });
        requestResyncLane();
        return "recovered";
      }

      if (last >= MAX_WIRE_COUNTER) {
        // Sequence exhaustion: checked, content-free, never wraps/reuses.
        sealTo({ kind: "sealed", generation: state.generation });
        requestResyncLane();
        return "recovered";
      }

      const incomingBytes = delivery.data.length;
      const pendingBytes = state.pendingBytes;
      const writeInFlightBytes = state.kind === "live" ? state.writeInFlightBytes : 0;
      const combined = writeInFlightBytes + pendingBytes + incomingBytes;
      if (!Number.isSafeInteger(combined) || combined > RENDER_PENDING_LIMIT_BYTES) {
        // Checked bound BEFORE allocation/retention/acknowledgement.
        sealTo({ kind: "sealed", generation: state.generation });
        requestResyncLane();
        return "recovered";
      }

      state.queue.push({
        bytes: delivery.data,
        firstSequence: first,
        sequence: last,
      });
      state.pendingBytes += incomingBytes;
      state.nextSequence = last + 1n;
      bytesAccepted += incomingBytes;
      trackHighWater();
      options.acknowledge(
        sessionId,
        delivery.generation,
        delivery.firstSequence,
        delivery.sequence
      );
      if (state.kind === "live") {
        scheduleDrain();
      }
      return "accepted";
    },

    completeWrite(writeToken) {
      if (state.kind !== "live" || state.writeGate === null) {
        noteRetiredCallback();
        return;
      }
      const gate = state.writeGate;
      if (gate.inert || gate.token !== writeToken) {
        noteRetiredCallback();
        return;
      }
      state.writeInFlightBytes = 0;
      gate.inert = true;
      state.writeGate = null;
      scheduleDrain();
    },

    sealGeneration() {
      if (state.kind === "replayPending" || state.kind === "live") {
        sealTo({ kind: "sealed", generation: state.generation });
      }
    },

    requestResync() {
      if (state.kind === "replayPending" || state.kind === "live") {
        sealTo({ kind: "sealed", generation: state.generation });
      }
      requestResyncLane();
    },

    dispose() {
      sealTo({ kind: "idle" });
    },

    reset() {
      sealTo({ kind: "idle" });
    },

    noteRejected() {
      inactiveOrStaleEventsRejected += 1;
    },

    noteLivenessRecovery() {
      replayPendingLivenessRecoveries += 1;
    },

    stateKind() {
      return state.kind;
    },

    generation() {
      return state.kind === "idle" ? null : state.generation;
    },

    snapshotSequence() {
      return state.kind === "replayPending" || state.kind === "live"
        ? state.snapshotSequence
        : null;
    },

    nextSequence() {
      return state.kind === "replayPending" || state.kind === "live"
        ? state.nextSequence
        : null;
    },

    pendingBytes() {
      return state.kind === "replayPending" || state.kind === "live"
        ? state.pendingBytes
        : 0;
    },

    writeInFlightBytes() {
      return state.kind === "live" ? state.writeInFlightBytes : 0;
    },

    isReplayPending() {
      return state.kind === "replayPending";
    },

    hasStrongReplayGate() {
      return state.kind === "replayPending" && state.replayGate !== null;
    },

    hasStrongWriteGate() {
      return state.kind === "live" && state.writeGate !== null;
    },

    metrics() {
      return {
        outputEventsReceived,
        inactiveOrStaleEventsRejected,
        bytesAccepted,
        bytesWritten,
        replayPendingBytes: state.kind === "replayPending" ? state.pendingBytes : 0,
        livePendingBytes: state.kind === "live" ? state.pendingBytes : 0,
        writeInFlightBytes: state.kind === "live" ? state.writeInFlightBytes : 0,
        combinedAdmissionHighWaterBytes: combinedHighWater,
        pendingHighWaterBytes: pendingHighWater,
        resyncCount,
        replayPendingLivenessRecoveries,
        snapshotReplayDurationMs,
        retiredWriteCallbacksIgnoredAfterDisposal,
        maxAnimationFrameLagMs,
      };
    },
  };

  return admission;
}
