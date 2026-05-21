import { describe, it, expect, beforeEach } from "vitest";

import type { ErrorLogEntry } from "../../shared/types";
import { errorModalStore, __resetErrorModalStoreForTests } from "./error-modal";

// Mirrors FRONTEND_QUEUE_CAP in error-modal.ts (module-private there). The
// store has no IPC import, so these cases need no mocks.
const FRONTEND_QUEUE_CAP = 200;

function mkEntry(id: number): ErrorLogEntry {
  return {
    timestamp: "2026-05-21 12:00:00.000",
    level: "ERROR",
    target: "agentscommander_lib::test",
    message: `error #${id}`,
  };
}

function mkEntries(count: number, startId = 0): ErrorLogEntry[] {
  return Array.from({ length: count }, (_, i) => mkEntry(startId + i));
}

describe("errorModalStore", () => {
  beforeEach(() => {
    __resetErrorModalStoreForTests();
  });

  it("starts closed", () => {
    expect(errorModalStore.open).toBe(false);
    expect(errorModalStore.current).toBeNull();
    expect(errorModalStore.total).toBe(0);
    expect(errorModalStore.index).toBe(0);
  });

  it("enqueue opens the modal at the first entry", () => {
    const [a, b] = mkEntries(2);
    errorModalStore.enqueue([a, b]);
    expect(errorModalStore.open).toBe(true);
    expect(errorModalStore.current).toBe(a);
    expect(errorModalStore.index).toBe(0);
    expect(errorModalStore.total).toBe(2);
  });

  it("dismissCurrent advances to the next entry", () => {
    const [a, b] = mkEntries(2);
    errorModalStore.enqueue([a, b]);
    errorModalStore.dismissCurrent();
    expect(errorModalStore.current).toBe(b);
    expect(errorModalStore.index).toBe(1);
    expect(errorModalStore.open).toBe(true);
  });

  it("dismissing the last entry closes the modal and resets index/total", () => {
    errorModalStore.enqueue(mkEntries(2));
    errorModalStore.dismissCurrent();
    errorModalStore.dismissCurrent();
    expect(errorModalStore.open).toBe(false);
    expect(errorModalStore.current).toBeNull();
    expect(errorModalStore.index).toBe(0);
    expect(errorModalStore.total).toBe(0);
  });

  it("enqueue while open grows total but keeps current (reference-identical)", () => {
    const [a, b] = mkEntries(2);
    errorModalStore.enqueue([a, b]);
    const before = errorModalStore.current;
    errorModalStore.enqueue([mkEntry(99)]);
    expect(errorModalStore.total).toBe(3);
    expect(errorModalStore.index).toBe(0);
    // H1 contract: the shown entry keeps its object identity, so ErrorModal's
    // createMemo stays quiet and does not steal focus / reset "Copied!".
    expect(errorModalStore.current).toBe(before);
    expect(errorModalStore.current).toBe(a);
  });

  it("enqueue([]) is a no-op", () => {
    errorModalStore.enqueue([]);
    expect(errorModalStore.open).toBe(false);
    expect(errorModalStore.total).toBe(0);
    expect(errorModalStore.current).toBeNull();
  });

  it("a fresh burst after a full dismiss starts a new 1 of N", () => {
    errorModalStore.enqueue(mkEntries(2));
    errorModalStore.dismissCurrent();
    errorModalStore.dismissCurrent();
    expect(errorModalStore.open).toBe(false);
    const burst = mkEntries(3, 100);
    errorModalStore.enqueue(burst);
    expect(errorModalStore.open).toBe(true);
    expect(errorModalStore.index).toBe(0);
    expect(errorModalStore.current).toBe(burst[0]);
    expect(errorModalStore.total).toBe(3);
  });

  it("caps the queue at FRONTEND_QUEUE_CAP, dropping the oldest entries", () => {
    const incoming = mkEntries(FRONTEND_QUEUE_CAP + 50);
    errorModalStore.enqueue(incoming);
    expect(errorModalStore.total).toBe(FRONTEND_QUEUE_CAP);
    // Oldest 50 dropped; the queue head is now the 50th original entry.
    expect(errorModalStore.current).toBe(incoming[50]);
  });

  it("on overflow shifts index down so current is preserved", () => {
    errorModalStore.enqueue(mkEntries(100));
    for (let i = 0; i < 50; i++) errorModalStore.dismissCurrent();
    const viewing = errorModalStore.current;
    expect(viewing).not.toBeNull();
    // Combined 100 + 150 = 250, drop 50 oldest. The viewed entry was at
    // index 50; index shifts down by 50 -> 0, same object reference.
    errorModalStore.enqueue(mkEntries(150, 1000));
    expect(errorModalStore.total).toBe(FRONTEND_QUEUE_CAP);
    expect(errorModalStore.index).toBe(0);
    expect(errorModalStore.current).toBe(viewing);
  });

  it("clamps index to 0 when the viewed entry is itself evicted", () => {
    errorModalStore.enqueue(mkEntries(60));
    for (let i = 0; i < 10; i++) errorModalStore.dismissCurrent();
    expect(errorModalStore.index).toBe(10);
    // Combined 60 + 300 = 360, drop 160 oldest. All 60 original entries are
    // evicted (160 > 59), including the one being viewed -> index clamps to 0.
    const flood = mkEntries(300, 1000);
    errorModalStore.enqueue(flood);
    expect(errorModalStore.total).toBe(FRONTEND_QUEUE_CAP);
    expect(errorModalStore.index).toBe(0);
    expect(errorModalStore.current).toBe(flood[100]);
  });
});
