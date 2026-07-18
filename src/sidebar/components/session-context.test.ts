import { describe, expect, it } from "vitest";
import type { AgentConfig } from "../../shared/types";
import { contextBadgeConfigured, contextBadgeText } from "./session-context";

function agent(over: Partial<AgentConfig> & { id: string }): AgentConfig {
  return {
    label: "Claude",
    command: "claude",
    color: "#d97757",
    envs: [],
    isolatedHome: false,
    ...over,
  };
}

describe("contextBadgeText", () => {
  it("renders a reading as a percentage (a_reading_renders_as_a_percentage)", () => {
    expect(contextBadgeText(42)).toBe("CTX 42%");
    expect(contextBadgeText(100)).toBe("CTX 100%");
  });

  // The highest-value test in this file: it is the only thing that stays red if
  // anyone writes `percent ? ... : "CTX N/A"`. A true 0% is a reading, and turning
  // it into N/A keeps every other case working and is invisible on screen.
  it("renders a real reading of zero as CTX 0%, never N/A (zero_is_a_reading_not_an_absence)", () => {
    expect(contextBadgeText(0)).toBe("CTX 0%");
  });

  it("renders both null and undefined as one unavailable state (null_and_undefined_both_render_unavailable)", () => {
    // Pins that unavailable is exactly one thing: an explicit null from the engine
    // and a key no event has spoken for are indistinguishable to the user.
    expect(contextBadgeText(null)).toBe("CTX N/A");
    expect(contextBadgeText(undefined)).toBe("CTX N/A");
  });

  it("is total (the_projection_is_total)", () => {
    for (const value of [0, 1, 50, 99, 100, null, undefined]) {
      expect(() => contextBadgeText(value)).not.toThrow();
      expect(contextBadgeText(value).length).toBeGreaterThan(0);
    }
  });
});

describe("contextBadgeConfigured", () => {
  it("shows the badge for an agent with a pattern (a_configured_agent_is_visible)", () => {
    expect(contextBadgeConfigured([agent({ id: "a", contextRegex: "x" })], "a")).toBe(true);
  });

  it("hides the badge for an agent with no pattern (an_agent_with_no_regex_is_not_visible)", () => {
    expect(contextBadgeConfigured([agent({ id: "a" })], "a")).toBe(false);
  });

  // The hand-edited-file case: without this, such an agent shows a permanent N/A.
  // This trim is a visibility test only; it never reaches a stored value.
  it("hides the badge for a whitespace-only pattern (a_whitespace_only_regex_is_not_visible)", () => {
    expect(contextBadgeConfigured([agent({ id: "a", contextRegex: "   " })], "a")).toBe(false);
  });

  it("hides the badge for a plain shell (a_null_agentId_is_not_visible)", () => {
    const agents = [agent({ id: "a", contextRegex: "x" })];
    expect(contextBadgeConfigured(agents, null)).toBe(false);
    expect(contextBadgeConfigured(agents, undefined)).toBe(false);
  });

  it("hides the badge for an unknown agent id without throwing (an_unknown_agentId_is_not_visible)", () => {
    expect(contextBadgeConfigured([agent({ id: "a", contextRegex: "x" })], "ghost")).toBe(false);
    expect(contextBadgeConfigured(undefined, "a")).toBe(false);
    expect(contextBadgeConfigured([], "a")).toBe(false);
  });

  // Pins #1031's hard rule at the only place #1033 could break it: two agents may
  // share a command while only one configures a pattern, so the gate must key by id.
  it("keys by agent id and never by command (the_gate_keys_by_id_and_not_by_command)", () => {
    const agents = [
      agent({ id: "claude-a", command: "claude", contextRegex: "x" }),
      agent({ id: "claude-b", command: "claude" }),
    ];
    expect(contextBadgeConfigured(agents, "claude-a")).toBe(true);
    expect(contextBadgeConfigured(agents, "claude-b")).toBe(false);
  });
});
