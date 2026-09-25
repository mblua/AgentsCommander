import { describe, expect, it } from "vitest";
import type { AcLoopSummary, LoopEventPayload } from "../shared/types";
import { formatLoopNextDue } from "./components/loop-modal-helpers";
import { loopToastFromEvent } from "./loop-event-toast";

function summary(overrides: Partial<AcLoopSummary> = {}): AcLoopSummary {
  return {
    id: "standup",
    name: "Standup",
    enabled: true,
    expr: "0 9 * * *",
    timezone: "UTC",
    targetKind: "workgroupCoordinator",
    workgroup: "room-1-dev-team",
    promptPreview: "",
    busyCoordinator: "waitUntilIdle",
    sessionStart: "fresh",
    path: "C:\Project\.ac\loops\standup",
    configPath: "C:\Project\.ac\loops\standup\loop.json",
    lastCheckedAt: null,
    lastDueAt: null,
    lastDeliveredAt: null,
    lastResult: null,
    pendingDueAt: null,
    lastMissedClosedAt: null,
    nextDueAt: "2026-09-26T12:00:00Z",
    ...overrides,
  };
}

function event(kind: string, overrides: Partial<LoopEventPayload> = {}): LoopEventPayload {
  return {
    kind,
    projectPath: "C:\Project",
    loopId: "standup",
    message: null,
    ...overrides,
  };
}

describe("loopToastFromEvent", () => {
  it("uses error toast styling for missed, skipped, and failed Loop events", () => {
    expect(loopToastFromEvent(event("missed"))?.className).toBe("toast-error");
    expect(loopToastFromEvent(event("skipped"))?.className).toBe("toast-error");
    expect(loopToastFromEvent(event("failed"))?.className).toBe("toast-error");
  });

  it("uses info toast styling for pending and delivered Loop events", () => {
    expect(loopToastFromEvent(event("pending"))?.className).toBe("toast-info");
    expect(loopToastFromEvent(event("delivered"))?.className).toBe("toast-info");
  });

  it("ignores config mutation events that are already represented in the project tree", () => {
    expect(loopToastFromEvent(event("updated"))).toBeNull();
  });
});

describe("loopToastFromEvent delivered", () => {
  const next = formatLoopNextDue("2026-09-26T12:00:00Z");

  it("names the Loop, the room number, and the next delivery", () => {
    expect(loopToastFromEvent(event("delivered", { summary: summary() }))?.message).toBe(
      `Loop "Standup" delivered to room 1 · next ${next}`,
    );
  });

  it("ignores the backend message", () => {
    const toast = loopToastFromEvent(
      event("delivered", { summary: summary(), message: "Loop prompt delivered" }),
    );
    expect(toast?.message).not.toContain("Loop prompt delivered");
  });

  it("never names the target agent", () => {
    const toast = loopToastFromEvent(
      event("delivered", { summary: summary(), message: "p:room-1/coordinator" }),
    );
    expect(toast?.message).not.toContain("coordinator");
    expect(toast?.message).not.toContain("p:room-1");
  });

  it("calls a legacy wg- workgroup a room", () => {
    const toast = loopToastFromEvent(event("delivered", { summary: summary({ workgroup: "wg-3-team" }) }));
    expect(toast?.message).toContain(" to room 3");
  });

  it("omits the room when the workgroup has no number", () => {
    const toast = loopToastFromEvent(event("delivered", { summary: summary({ workgroup: "room-x" }) }));
    expect(toast?.message).toBe(`Loop "Standup" delivered · next ${next}`);
  });

  it("omits the next part when nextDueAt is null", () => {
    const toast = loopToastFromEvent(event("delivered", { summary: summary({ nextDueAt: null }) }));
    expect(toast?.message).toBe(`Loop "Standup" delivered to room 1`);
  });

  it("omits the next part when nextDueAt is not a valid date", () => {
    const toast = loopToastFromEvent(event("delivered", { summary: summary({ nextDueAt: "garbage" }) }));
    expect(toast?.message).not.toContain("next");
    expect(toast?.message).not.toContain("Invalid Date");
  });

  it("falls back to the Loop id without a summary", () => {
    expect(loopToastFromEvent(event("delivered"))?.message).toBe(`Loop "standup" delivered`);
  });
});
