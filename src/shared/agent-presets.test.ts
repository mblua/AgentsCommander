import { describe, expect, it } from "vitest";
import { AGENT_PRESETS, AGENT_PRESET_MAP } from "./agent-presets";

describe("agent presets (#529)", () => {
  it("carries the default instructions filename on each built-in preset", () => {
    expect(AGENT_PRESET_MAP.claude.instructionsFilename).toBe("CLAUDE.md");
    expect(AGENT_PRESET_MAP.codex.instructionsFilename).toBe("AGENTS.md");
  });

  it("ships Hermes / Cursor CLI / Pi and no longer ships Gemini (#766)", () => {
    // Gemini removed from the catalog (backend gemini plumbing is untouched;
    // existing user agents whose command is `gemini` are unaffected).
    expect(AGENT_PRESET_MAP.gemini).toBeUndefined();
    expect(AGENT_PRESETS.some((p) => p.key === "gemini")).toBe(false);

    expect(AGENT_PRESET_MAP.hermes?.command).toBe("hermes");
    expect(AGENT_PRESET_MAP.hermes?.instructionsFilename).toBe("AGENTS.md");
    // Cursor CLI's executable is `agent`, not `cursor`.
    expect(AGENT_PRESET_MAP.cursor?.command).toBe("agent");
    expect(AGENT_PRESET_MAP.cursor?.instructionsFilename).toBe("AGENTS.md");
    expect(AGENT_PRESET_MAP.pi?.command).toBe("pi");
    expect(AGENT_PRESET_MAP.pi?.instructionsFilename).toBe("AGENTS.md");

    // Label drives the onboarding card + settings quick-add button text.
    expect(AGENT_PRESETS.find((p) => p.key === "cursor")?.label).toBe("Cursor CLI");
  });

  it("ships an OpenCode preset with the bare `opencode` command and AGENTS.md", () => {
    const opencode = AGENT_PRESETS.find((p) => p.key === "opencode");
    expect(opencode).toBeTruthy();
    expect(opencode?.label).toBe("OpenCode");
    expect(opencode?.config.command).toBe("opencode");
    expect(opencode?.config.instructionsFilename).toBe("AGENTS.md");
    // Reachable via the quick-add lookup the Config Screen buttons use.
    expect(AGENT_PRESET_MAP.opencode?.command).toBe("opencode");
    expect(AGENT_PRESET_MAP.opencode?.instructionsFilename).toBe("AGENTS.md");
  });
});
