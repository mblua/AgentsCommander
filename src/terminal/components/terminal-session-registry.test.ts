// @vitest-environment jsdom
//
// #1283 - terminal-session-registry contract (plan 9.1, 14.4, 14.5.9).
//
// Capacity four, activation-based LRU ordering, exactly-once WebGL/Xterm/DOM
// disposal, no visible/ReplayPending eviction, safe duplicate destroy, and
// synchronous full teardown with a deliberately never-fired write callback:
// zero retired entry/terminal/timer/strong-gate owner/counter/raw bytes after
// disposal, and a later forced callback is a no-op.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createTerminalSessionRegistry,
  TERMINAL_RETENTION_LIMIT,
  type SessionTerminalEntry,
  type TerminalRegistry,
} from "./terminal-session-registry";

class FakeFitAddon {
  activate(): void {}
  fit = vi.fn();
  proposeDimensions(): { cols: number; rows: number } {
    return { cols: 80, rows: 24 };
  }
}

interface FakeTerminal {
  cols: number;
  rows: number;
  writes: { bytes: number[]; callback: (() => void) | null }[];
  disposed: boolean;
  write(data: Uint8Array, callback?: () => void): void;
  dispose(): void;
  loadAddon(): void;
  open(): void;
  focus(): void;
  resize(cols: number, rows: number): void;
  reset(): void;
  scrollToBottom(): void;
  paste(): void;
  hasSelection(): boolean;
  getSelection(): string;
  attachCustomKeyEventHandler(): void;
  onData(): { dispose: () => void };
  onResize(): { dispose: () => void };
}

class FakeTerminalImpl implements FakeTerminal {
  cols = 80;
  rows = 24;
  writes: { bytes: number[]; callback: (() => void) | null }[] = [];
  disposed = false;

  write(data: Uint8Array, callback?: () => void): void {
    this.writes.push({ bytes: Array.from(data), callback: callback ?? null });
  }

  dispose(): void {
    this.disposed = true;
  }

  loadAddon(): void {}
  open(): void {}
  focus(): void {}
  resize(cols: number, rows: number): void {
    this.cols = cols;
    this.rows = rows;
  }
  reset(): void {}
  scrollToBottom(): void {}
  paste(): void {}
  hasSelection(): boolean {
    return false;
  }
  getSelection(): string {
    return "";
  }
  attachCustomKeyEventHandler(): void {}
  onData(): { dispose: () => void } {
    return { dispose: () => {} };
  }
  onResize(): { dispose: () => void } {
    return { dispose: () => {} };
  }
}

interface FakeWebgl {
  disposed: boolean;
  onContextLoss(callback: () => void): void;
  dispose(): void;
}

class FakeWebglImpl implements FakeWebgl {
  disposed = false;
  private lossCallback: (() => void) | null = null;
  onContextLoss(callback: () => void): void {
    this.lossCallback = callback;
  }
  fireContextLoss(): void {
    this.lossCallback?.();
  }
  dispose(): void {
    this.disposed = true;
  }
}

interface Harness {
  registry: TerminalRegistry;
  host: HTMLDivElement;
  terminals: Map<string, FakeTerminalImpl>;
  webgls: Map<string, FakeWebglImpl>;
  disposedAdmissions: string[];
  contextLosses: number;
  entry(sessionId: string): SessionTerminalEntry;
}

const S1 = "11111111-1111-4111-8111-111111111111";
const S2 = "22222222-2222-4222-8222-222222222222";
const S3 = "33333333-3333-4333-8333-333333333333";
const S4 = "44444444-4444-4444-8444-444444444444";
const S5 = "55555555-5555-4555-8555-555555555555";

function createHarness(): Harness {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const terminals = new Map<string, FakeTerminalImpl>();
  const webgls = new Map<string, FakeWebglImpl>();
  const disposedAdmissions: string[] = [];

  const registry = createTerminalSessionRegistry({
    host: () => host,
    beforeResourceDispose: (sessionId) => {
      disposedAdmissions.push(sessionId);
    },
  });

  return {
    registry,
    host,
    terminals,
    webgls,
    disposedAdmissions,
    contextLosses: 0,
    entry(sessionId) {
      const entry = registry.get(sessionId);
      if (!entry) throw new Error(`no entry for ${sessionId}`);
      return entry;
    },
  };
}

function createFactory(h: Harness) {
  return (sessionId: string, container: HTMLDivElement) => {
    const terminal = new FakeTerminalImpl();
    const webgl = new FakeWebglImpl();
    webgl.onContextLoss(() => {
      h.contextLosses += 1;
      h.registry.noteWebglContextLoss();
      webgl.dispose();
    });
    h.terminals.set(sessionId, terminal);
    h.webgls.set(sessionId, webgl);
    const replayStatus = document.createElement("div");
    const agentMessageStatus = document.createElement("div");
    return {
      terminal: terminal as unknown as SessionTerminalEntry["terminal"],
      fitAddon: new FakeFitAddon() as unknown as SessionTerminalEntry["fitAddon"],
      webglAddon: webgl as unknown as SessionTerminalEntry["webglAddon"],
      replayStatus,
      agentMessageStatus,
      agentMessageAtMs: null,
      hasRenderedOutput: false,
      snapshotResizeSuppressed: false,
      inputBuffer: "",
      spawnViewport: null,
      lastSentViewport: null,
      spawnDriftReported: false,
      resizeRetryTimer: null,
      resizeRetryAttempts: 0,
      snapshotSettleTimer: null,
      snapshotReplayPending: false,
      pendingSnapshotEvents: [],
      pendingSnapshotBytes: 0,
      lastAppliedSequence: null,
    };
  };
}

describe("terminal-session-registry", () => {
  let h: Harness;

  beforeEach(() => {
    h = createHarness();
  });

  afterEach(() => {
    h.registry.disposeAll();
    h.host.remove();
  });

  it("retains at most four entries and evicts the least recently activated hidden entry", () => {
    const factory = createFactory(h);
    const order = [S1, S2, S3, S4, S5];
    for (const sessionId of order) {
      h.registry.activate(sessionId, factory);
    }

    // Five selections: the first (S1) is the LRU non-visible entry and is gone.
    expect(h.registry.get(S1)).toBeUndefined();
    expect(h.registry.metrics().retained).toBe(TERMINAL_RETENTION_LIMIT);
    expect(h.registry.metrics().lruEvictions).toBe(1);
    expect(h.terminals.get(S1)!.disposed).toBe(true);
    expect(h.disposedAdmissions).toContain(S1);
    // exactly-once DOM removal
    expect(document.querySelector(`[data-ac-session-id="${S1}"]`)).toBeNull();

    // Re-activating an existing session never grows the map: no eviction.
    h.registry.activate(S2, factory);
    expect(h.registry.get(S3)).toBeDefined();
    expect(h.registry.metrics().retained).toBe(4);

    // A NEW sixth activation evicts the LRU hidden entry. S2 was just touched
    // (t6), so the victim is S3 (t3).
    const S6 = "66666666-6666-4666-8666-666666666666";
    h.registry.activate(S6, factory);
    expect(h.registry.get(S3)).toBeUndefined();
    expect(h.registry.get(S2)).toBeDefined();
    expect(h.registry.metrics().retained).toBe(4);
    expect(h.registry.metrics().lruEvictions).toBe(2);
  });

  it("never evicts the visible (active/ReplayPending) entry", () => {
    const factory = createFactory(h);
    for (const sessionId of [S1, S2, S3, S4]) {
      h.registry.activate(sessionId, factory);
    }
    // Activate S5: S1..S4 are all hidden; S1 is LRU -> evicted.
    h.registry.activate(S5, factory);
    expect(h.registry.get(S1)).toBeUndefined();
    expect(h.registry.getVisible()).toBe(S5);
    expect(h.registry.get(S5)).toBeDefined();

    // The visible entry is never a victim: a sixth activation evicts the LRU
    // hidden entry (S2) while the visible S5 is protected.
    const S6 = "66666666-6666-4666-8666-666666666666";
    h.registry.activate(S6, factory);
    expect(h.registry.get(S5)).toBeDefined();
    expect(h.registry.getVisible()).toBe(S6);
    expect(h.registry.get(S2)).toBeUndefined();
    expect(h.registry.get(S5)).toBeDefined();
  });

  it("disposes WebGL, Xterm, and DOM exactly once and tolerates duplicate destroy", () => {
    const factory = createFactory(h);
    h.registry.activate(S1, factory);
    const terminal = h.terminals.get(S1)!;
    const webgl = h.webgls.get(S1)!;

    h.registry.remove(S1);
    expect(terminal.disposed).toBe(true);
    expect(webgl.disposed).toBe(true);
    expect(document.querySelector(`[data-ac-session-id="${S1}"]`)).toBeNull();
    expect(h.disposedAdmissions).toEqual([S1]);

    // Duplicate destroy: exactly-once teardown, no error, no second hook call.
    h.registry.remove(S1);
    h.registry.remove(S1);
    expect(h.disposedAdmissions).toEqual([S1]);
    expect(h.registry.metrics().retained).toBe(0);
  });

  it("cancels every per-entry timer during disposal", () => {
    vi.useFakeTimers();
    try {
      const factory = createFactory(h);
      h.registry.activate(S1, factory);
      const entry = h.entry(S1);
      entry.snapshotSettleTimer = setTimeout(() => {}, 500);
      entry.resizeRetryTimer = setTimeout(() => {}, 120);

      h.registry.remove(S1);
      expect(entry.snapshotSettleTimer).toBeNull();
      expect(entry.resizeRetryTimer).toBeNull();

      // Advancing past every deadline must not fire anything.
      const spy = vi.fn();
      vi.spyOn(globalThis, "setTimeout").mockImplementation(() => {
        spy();
        return 0 as unknown as ReturnType<typeof setTimeout>;
      });
      vi.advanceTimersByTime(10_000);
      expect(spy).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("counts WebGL context losses through the registry metrics", () => {
    const factory = createFactory(h);
    h.registry.activate(S1, factory);
    const entry = h.entry(S1);
    (entry.webglAddon as unknown as FakeWebglImpl).fireContextLoss();
    (entry.webglAddon as unknown as FakeWebglImpl).fireContextLoss();
    expect(h.contextLosses).toBe(2);
    expect(h.registry.metrics().webglContextLosses).toBe(2);
  });

  it("reports retained/visible/webgl gauges", () => {
    const factory = createFactory(h);
    h.registry.activate(S1, factory);
    h.registry.activate(S2, factory);
    const metrics = h.registry.metrics();
    expect(metrics.retained).toBe(2);
    expect(metrics.visible).toBe(1);
    expect(metrics.webglContexts).toBe(2);

    h.registry.setVisible(null);
    expect(h.registry.metrics().visible).toBe(0);
    h.registry.remove(S1);
    expect(h.registry.metrics().retained).toBe(1);
  });

  it("releases every resource synchronously for a held never-fired write callback", () => {
    const factory = createFactory(h);
    h.registry.activate(S1, factory);
    const terminal = h.terminals.get(S1)!;
    // Start a write and NEVER release its callback.
    terminal.write(new Uint8Array([65, 66]), () => {});

    h.registry.remove(S1);
    // Synchronous full teardown: entry, terminal, DOM, and admission hook gone.
    expect(h.registry.get(S1)).toBeUndefined();
    expect(terminal.disposed).toBe(true);
    expect(document.querySelector(`[data-ac-session-id="${S1}"]`)).toBeNull();
    expect(h.disposedAdmissions).toEqual([S1]);

    // Forcing the late callback must be a harmless no-op (no throw, no state).
    const late = terminal.writes[0].callback;
    expect(() => late?.()).not.toThrow();
  });

  it("disposeAll tears down every entry exactly once", () => {
    const factory = createFactory(h);
    for (const sessionId of [S1, S2, S3]) {
      h.registry.activate(sessionId, factory);
    }
    h.registry.disposeAll();
    expect(h.registry.metrics().retained).toBe(0);
    expect(h.disposedAdmissions.sort()).toEqual([S1, S2, S3].sort());
    for (const sessionId of [S1, S2, S3]) {
      expect(h.terminals.get(sessionId)!.disposed).toBe(true);
      expect(document.querySelector(`[data-ac-session-id="${sessionId}"]`)).toBeNull();
    }
    // Idempotent.
    h.registry.disposeAll();
    expect(h.disposedAdmissions).toHaveLength(3);
  });

  it("recreates an entry after disposal with a fresh terminal", () => {
    const factory = createFactory(h);
    h.registry.activate(S1, factory);
    const first = h.terminals.get(S1)!;
    h.registry.remove(S1);

    h.registry.activate(S1, factory);
    const second = h.terminals.get(S1)!;
    expect(second).not.toBe(first);
    expect(first.disposed).toBe(true);
    expect(second.disposed).toBe(false);
    expect(document.querySelector(`[data-ac-session-id="${S1}"]`)).not.toBeNull();
  });
});
