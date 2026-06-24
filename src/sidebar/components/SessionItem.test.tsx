// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SessionItem from "./SessionItem";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { settingsStore } from "../../shared/stores/settings";
import type { AgentConfig, AppSettings, CodingAgentProfilesConfig, Session } from "../../shared/types";

function agentConfig(id: string, label: string, command: string): AgentConfig {
  return {
    id,
    label,
    command,
    color: "#888888",
    envs: [],
    isolatedHome: false,
  };
}

// codex = agents[0] = primigenio; claude is the second coding agent.
const TWO_AGENTS: AgentConfig[] = [
  agentConfig("codex", "Codex", "codex"),
  agentConfig("claude", "Claude Code", "claude"),
];

function profiles(
  labelsByAgent: CodingAgentProfilesConfig["profileLabelsByAgent"],
): CodingAgentProfilesConfig {
  return {
    schemaVersion: 2,
    profileSlots: { A: { label: "" }, B: { label: "" } },
    defaultProfileByAgent: {},
    profileLabelsByAgent: labelsByAgent,
    profilesByAgent: {},
  };
}

async function renderRow(sessionProps: Partial<Session>, settings: AppSettings) {
  const fake = new FakeTransport();
  fake.resolve("get_settings", settings);
  const rendered = renderWithFakeTransport(
    () => <SessionItem session={session(sessionProps)} isActive={false} />,
    fake,
  );
  await settingsStore.load();
  return rendered;
}

function badge(root: ParentNode): HTMLElement {
  const el = root.querySelector<HTMLElement>(".profile-badge");
  if (!el) throw new Error("profile-badge not rendered");
  return el;
}

describe("SessionItem profile badge tooltip (#548)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("names the effective profile with the agent's OWN label", async () => {
    const settings = baseSettings({
      agents: TWO_AGENTS,
      codingAgentProfiles: profiles({ codex: { B: "turbo" } }),
    });
    const rendered = await renderRow(
      {
        agentId: "codex",
        agentLabel: "Codex",
        requestedProfile: "B",
        effectiveProfile: "B",
        profileFallbackApplied: false,
      },
      settings,
    );
    try {
      await waitFor(() => expect(badge(rendered.root).getAttribute("title")).toBe("B-TURBO"));
      // The badge glyph stays the bare letter; only the tooltip carries the name.
      expect(badge(rendered.root).textContent).toBe("B");
    } finally {
      rendered.cleanup();
    }
  });

  it("inherits the primigenio (agents[0]) label when the agent has no own label", async () => {
    const settings = baseSettings({
      agents: TWO_AGENTS, // codex = primigenio holds the only B label
      codingAgentProfiles: profiles({ codex: { B: "turbo" } }),
    });
    const rendered = await renderRow(
      {
        agentId: "claude",
        agentLabel: "Claude Code",
        requestedProfile: "B",
        effectiveProfile: "B",
        profileFallbackApplied: false,
      },
      settings,
    );
    try {
      // claude has no own B → inherits the primigenio (codex).
      await waitFor(() => expect(badge(rendered.root).getAttribute("title")).toBe("B-TURBO"));
    } finally {
      rendered.cleanup();
    }
  });

  it("stays independent once the agent sets its own label (editing claude never touches codex)", async () => {
    const settings = baseSettings({
      agents: TWO_AGENTS,
      codingAgentProfiles: profiles({ codex: { B: "turbo" }, claude: { B: "zen" } }),
    });
    const claudeRow = await renderRow(
      {
        agentId: "claude",
        agentLabel: "Claude Code",
        requestedProfile: "B",
        effectiveProfile: "B",
        profileFallbackApplied: false,
      },
      settings,
    );
    try {
      await waitFor(() => expect(badge(claudeRow.root).getAttribute("title")).toBe("B-ZEN"));
    } finally {
      claudeRow.cleanup();
    }
    // codex is unaffected by claude's own override.
    const codexRow = await renderRow(
      {
        agentId: "codex",
        agentLabel: "Codex",
        requestedProfile: "B",
        effectiveProfile: "B",
        profileFallbackApplied: false,
      },
      settings,
    );
    try {
      await waitFor(() => expect(badge(codexRow.root).getAttribute("title")).toBe("B-TURBO"));
    } finally {
      codexRow.cleanup();
    }
  });

  it("on a fallback badge, names the EFFECTIVE letter while the glyph keeps the arrow", async () => {
    const settings = baseSettings({
      agents: TWO_AGENTS,
      codingAgentProfiles: profiles({}),
    });
    const rendered = await renderRow(
      {
        agentId: "codex",
        agentLabel: "Codex",
        requestedProfile: "B",
        effectiveProfile: "A",
        profileFallbackApplied: true,
      },
      settings,
    );
    try {
      await waitFor(() => {
        // Glyph shows the requested->effective arrow.
        expect(badge(rendered.root).textContent).toBe("B->A");
        // Tooltip names the EFFECTIVE letter A; no A name resolves → bare "A".
        expect(badge(rendered.root).getAttribute("title")).toBe("A");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("resolves the effective letter's name on a fallback badge when one is set", async () => {
    const settings = baseSettings({
      agents: TWO_AGENTS,
      codingAgentProfiles: profiles({ codex: { A: "baseline" } }),
    });
    const rendered = await renderRow(
      {
        agentId: "codex",
        agentLabel: "Codex",
        requestedProfile: "B",
        effectiveProfile: "A",
        profileFallbackApplied: true,
      },
      settings,
    );
    try {
      await waitFor(() => {
        expect(badge(rendered.root).textContent).toBe("B->A");
        expect(badge(rendered.root).getAttribute("title")).toBe("A-BASELINE");
      });
    } finally {
      rendered.cleanup();
    }
  });
});

describe("SessionItem profile-outdated badge (#592)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("shows the Reload badge when profileOutdated is true", async () => {
    const settings = baseSettings({ agents: TWO_AGENTS, codingAgentProfiles: profiles({}) });
    const rendered = await renderRow(
      { agentId: "codex", agentLabel: "Codex", profileOutdated: true },
      settings,
    );
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector(".profile-outdated-badge")).not.toBeNull(),
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("hides the Reload badge when profileOutdated is falsy", async () => {
    const settings = baseSettings({ agents: TWO_AGENTS, codingAgentProfiles: profiles({}) });
    const rendered = await renderRow(
      { agentId: "codex", agentLabel: "Codex", profileOutdated: false },
      settings,
    );
    try {
      // The agent badge renders, so the meta container is present; the outdated
      // badge specifically must not be.
      await waitFor(() => expect(rendered.root.querySelector(".agent-badge")).not.toBeNull());
      expect(rendered.root.querySelector(".profile-outdated-badge")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
