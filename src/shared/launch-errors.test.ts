import { describe, expect, it } from "vitest";
import { launchErrorMessage } from "./launch-errors";

describe("launchErrorMessage", () => {
  it("maps the Resource Monitor cap rejection to a friendly, actionable message with counts", () => {
    const raw = "Resource Monitor cap reached: 16/16 agent groups are active";
    expect(launchErrorMessage(raw)).toBe(
      "Resource Monitor cap reached (16/16). Close an agent or raise the limit in Settings > Resources."
    );
  });

  it("reads the cap message off an Error instance", () => {
    const err = new Error("Resource Monitor cap reached: 3/8 agent groups are active");
    expect(launchErrorMessage(err)).toBe(
      "Resource Monitor cap reached (3/8). Close an agent or raise the limit in Settings > Resources."
    );
  });

  it("falls back gracefully when counts are absent", () => {
    expect(launchErrorMessage("Resource Monitor cap reached")).toBe(
      "Resource Monitor cap reached. Close an agent or raise the limit in Settings > Resources."
    );
  });

  it("passes non-cap failures through verbatim (never swallowed)", () => {
    expect(launchErrorMessage("boom: disk full")).toBe("boom: disk full");
    expect(launchErrorMessage(new Error("permission denied"))).toBe("permission denied");
  });

  it("returns a sensible fallback for empty/nullish errors", () => {
    expect(launchErrorMessage("")).toBe("Failed to start agent.");
    expect(launchErrorMessage(undefined)).toBe("Failed to start agent.");
  });
});
