import { describe, expect, it } from "vitest";
import type { LoopEventPayload } from "../shared/types";
import { loopToastFromEvent } from "./loop-event-toast";

function event(kind: string): LoopEventPayload {
  return {
    kind,
    projectPath: "C:\\Project",
    loopId: "standup",
    message: null,
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
