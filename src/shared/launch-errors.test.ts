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

  const UNRESOLVED =
    "unresolved_coding_agent_reference: 'claude-old' matches no configured coding agent";
  const FIX = "Right-click the session, choose Coding Agent, pick one, then restart.";

  it("maps_the_unresolved_reference_to_plain_words", () => {
    expect(launchErrorMessage(UNRESOLVED)).toBe(
      `Can't restart this session: its coding agent 'claude-old' is no longer in Settings. ${FIX}`
    );
    expect(launchErrorMessage(new Error(UNRESOLVED))).toBe(launchErrorMessage(UNRESOLVED));
  });

  it("keeps_the_vanished_agent_id_in_the_message", () => {
    const text = launchErrorMessage(UNRESOLVED);
    expect(text).toContain("'claude-old'");
    expect(text).not.toContain("unresolved_coding_agent_reference");
  });

  it("falls_back_when_the_id_cannot_be_parsed", () => {
    const expected = `Can't restart this session: its coding agent is no longer in Settings. ${FIX}`;
    for (const raw of [
      "unresolved_coding_agent_reference: matches no configured coding agent",
      "unresolved_coding_agent_reference: '' matches no configured coding agent",
    ]) {
      const text = launchErrorMessage(raw);
      expect(text).toBe(expected);
      expect(text).not.toContain("unresolved_coding_agent_reference");
    }
  });

  it("leaves_other_errors_alone", () => {
    expect(launchErrorMessage("boom: 'x' unresolved_coding_agent_reference")).toBe(
      "boom: 'x' unresolved_coding_agent_reference"
    );
    expect(launchErrorMessage("Resource Monitor cap reached: 16/16 agent groups are active")).toBe(
      "Resource Monitor cap reached (16/16). Close an agent or raise the limit in Settings > Resources."
    );
  });
});
