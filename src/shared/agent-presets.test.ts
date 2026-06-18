import { describe, expect, it } from "vitest";
import { AGENT_PRESETS, AGENT_PRESET_MAP } from "./agent-presets";

describe("agent presets (#529)", () => {
  it("carries the default instructions filename on each built-in preset", () => {
    expect(AGENT_PRESET_MAP.claude.instructionsFilename).toBe("CLAUDE.md");
    expect(AGENT_PRESET_MAP.codex.instructionsFilename).toBe("AGENTS.md");
    expect(AGENT_PRESET_MAP.gemini.instructionsFilename).toBe("GEMINI.md");
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
