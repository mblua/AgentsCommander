// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SessionItem from "./SessionItem";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
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

describe("SessionItem coordinator repo folder menu (#708)", () => {
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

  function repoMenuItems(sessionId: string): HTMLElement[] {
    return Array.from(
      document.querySelectorAll<HTMLElement>(`[data-ac-testid^="session.${sessionId}.menu.repo."]`),
    );
  }

  function repoMenuLabel(item: HTMLElement): string | null {
    return item.querySelector<HTMLElement>(".session-context-repo-label")?.textContent ?? null;
  }

  it("renders one folder action per valid coordinator repo and opens duplicate labels by index", async () => {
    const repos = [
      { label: "AgentsCommander", sourcePath: "C:\\Project\\.ac\\wg-7-dev-team\\repo-AgentsCommander", branch: "feature/x", dirty: null },
      { label: "docs", sourcePath: "C:\\Project\\.ac\\wg-7-dev-team\\repo-docs-primary", branch: null, dirty: null },
      { label: "docs", sourcePath: "C:\\Project\\.ac\\wg-7-dev-team\\repo-docs-secondary", branch: "docs-alt", dirty: null },
      { label: "cli", sourcePath: "C:\\Project\\.ac\\wg-7-dev-team\\repo-cli", branch: null, dirty: null },
      { label: "mobile", sourcePath: "C:\\Project\\.ac\\wg-7-dev-team\\repo-mobile", branch: "main", dirty: null },
    ];
    const fake = new FakeTransport();
    fake.resolve("open_in_explorer", null);

    const rendered = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({
            id: "coord-1",
            name: "wg-7-dev-team/architect",
            isCoordinator: true,
            gitRepos: repos,
          })}
          isActive={false}
        />
      ),
      fake,
    );
    try {
      contextMenu(rendered.root.querySelector('[data-ac-testid="session.coord-1"]')!);

      await waitFor(() => {
        const items = repoMenuItems("coord-1");
        expect(items).toHaveLength(repos.length);
        expect(items.map(repoMenuLabel)).toEqual(["AgentsCommander", "docs", "docs", "cli", "mobile"]);
        expect(repoMenuLabel(items[0])).toBe("AgentsCommander");
        expect(items[0].textContent).not.toContain("Open");
        expect(items[0].textContent).not.toContain("feature/x");
        expect(repoMenuLabel(items[1])).toBe("docs");
        expect(repoMenuLabel(items[2])).toBe("docs");
      });

      click(document.querySelector('[data-ac-testid="session.coord-1.menu.repo.2"]')!);

      await waitFor(() =>
        expect(fake.lastCall("open_in_explorer")?.args).toEqual({
          path: "C:\\Project\\.ac\\wg-7-dev-team\\repo-docs-secondary",
        }),
      );
      expect(document.querySelector('[data-ac-testid="session.coord-1.menu"]')).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("filters malformed repo entries before rendering folder actions", async () => {
    const repos = [
      { label: "empty", sourcePath: "", branch: null },
      { label: "missing", branch: null },
      { label: "spaces", sourcePath: "   ", branch: null },
      { label: "valid", sourcePath: "C:\\Project\\.ac\\wg-7-dev-team\\repo-valid", branch: null },
    ] as unknown as Session["gitRepos"];
    const fake = new FakeTransport();
    fake.resolve("open_in_explorer", null);

    const rendered = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({
            id: "coord-filtered",
            name: "wg-7-dev-team/architect",
            isCoordinator: true,
            gitRepos: repos,
          })}
          isActive={false}
        />
      ),
      fake,
    );
    try {
      contextMenu(rendered.root.querySelector('[data-ac-testid="session.coord-filtered"]')!);

      await waitFor(() => {
        const items = repoMenuItems("coord-filtered");
        expect(items).toHaveLength(1);
        expect(repoMenuLabel(items[0])).toBe("valid");
      });

      click(document.querySelector('[data-ac-testid="session.coord-filtered.menu.repo.0"]')!);

      await waitFor(() =>
        expect(fake.lastCall("open_in_explorer")?.args).toEqual({
          path: "C:\\Project\\.ac\\wg-7-dev-team\\repo-valid",
        }),
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("hides repo folder actions for non-coordinators and coordinators without repos", async () => {
    const fake = new FakeTransport();
    fake.resolve("open_in_explorer", null);

    const nonCoordinator = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({
            id: "member-1",
            isCoordinator: false,
            gitRepos: [
              { label: "AgentsCommander", sourcePath: "C:\\Project\\.ac\\wg-7-dev-team\\repo-AgentsCommander", branch: null, dirty: null },
            ],
          })}
          isActive={false}
        />
      ),
      fake,
    );
    try {
      contextMenu(nonCoordinator.root.querySelector('[data-ac-testid="session.member-1"]')!);
      await waitFor(() => expect(document.querySelector('[data-ac-testid="session.member-1.menu"]')).not.toBeNull());
      expect(repoMenuItems("member-1")).toHaveLength(0);
    } finally {
      nonCoordinator.cleanup();
      document.body.replaceChildren();
    }

    const noRepoCoordinator = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({
            id: "coord-empty",
            isCoordinator: true,
            gitRepos: [],
          })}
          isActive={false}
        />
      ),
      fake,
    );
    try {
      contextMenu(noRepoCoordinator.root.querySelector('[data-ac-testid="session.coord-empty"]')!);
      await waitFor(() => expect(document.querySelector('[data-ac-testid="session.coord-empty.menu"]')).not.toBeNull());
      expect(repoMenuItems("coord-empty")).toHaveLength(0);
    } finally {
      noRepoCoordinator.cleanup();
    }
  });

  it("renders the optional extra context action only when provided", async () => {
    const fake = new FakeTransport();
    fake.resolve("open_in_explorer", null);
    const onSelect = vi.fn();

    const withExtra = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({ id: "with-extra" })}
          isActive={false}
          extraContextAction={{
            label: "Delete",
            class: "context-option-danger",
            icon: <span class="session-context-trash-icon" />,
            testId: "session.extra.delete",
            onSelect,
          }}
        />
      ),
      fake,
    );
    try {
      contextMenu(withExtra.root.querySelector('[data-ac-testid="session.with-extra"]')!);
      await waitFor(() => expect(document.querySelector('[data-ac-testid="session.with-extra.menu"]')).not.toBeNull());
      const action = document.querySelector<HTMLButtonElement>('[data-ac-testid="session.extra.delete"]');
      expect(action).not.toBeNull();
      expect(action!.textContent?.trim()).toBe("Delete");

      click(action!);

      expect(onSelect).toHaveBeenCalledTimes(1);
      expect(document.querySelector('[data-ac-testid="session.with-extra.menu"]')).toBeNull();
    } finally {
      withExtra.cleanup();
      document.body.replaceChildren();
    }

    const withoutExtra = renderWithFakeTransport(
      () => <SessionItem session={session({ id: "without-extra" })} isActive={false} />,
      fake,
    );
    try {
      contextMenu(withoutExtra.root.querySelector('[data-ac-testid="session.without-extra"]')!);
      await waitFor(() => expect(document.querySelector('[data-ac-testid="session.without-extra.menu"]')).not.toBeNull());
      expect(document.querySelector('[data-ac-testid="session.extra.delete"]')).toBeNull();
      expect(document.querySelector('[data-ac-testid="session.without-extra.menu"]')?.textContent).not.toContain("Delete");
    } finally {
      withoutExtra.cleanup();
    }
  });

  it("does not open a context menu for inactive coordinators even when repos exist", async () => {
    const fake = new FakeTransport();
    fake.resolve("open_in_explorer", null);

    const rendered = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({
            id: "inactive-coord-1",
            name: "wg-7-dev-team/architect",
            isCoordinator: true,
            gitRepos: [
              { label: "AgentsCommander", sourcePath: "C:\\Project\\.ac\\wg-7-dev-team\\repo-AgentsCommander", branch: null, dirty: null },
            ],
          })}
          isActive={false}
        />
      ),
      fake,
    );
    try {
      contextMenu(rendered.root.querySelector('[data-ac-testid="session.inactive-coord-1"]')!);
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(document.querySelector('[data-ac-testid="session.inactive-coord-1.menu"]')).toBeNull();
      expect(repoMenuItems("inactive-coord-1")).toHaveLength(0);
      expect(fake.lastCall("open_in_explorer")).toBeUndefined();
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
      // badge specifically must not be. #1167 moved the badge to the Coordinator
      // row's class pair.
      await waitFor(() =>
        expect(rendered.root.querySelector(".ac-discovery-badge.agent")).not.toBeNull(),
      );
      expect(rendered.root.querySelector(".profile-outdated-badge")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});

// #1167 - the badge must be one constant style in every row. jsdom does not apply
// sidebar.css, so the pin is on the emitted classes and attributes, which is what
// the CSS keys off: exactly `ac-discovery-badge agent`, no data-agent, no running.
describe("SessionItem coding-agent badge is style-invariant (#1167)", () => {
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

  const CASES: { name: string; props: Partial<Session> }[] = [
    { name: "live session, preset label", props: { agentId: "codex", agentLabel: "Codex", status: "running" } },
    { name: "live session, custom label", props: { agentId: "claude", agentLabel: "Isolated Claude", status: "running" } },
    { name: "exited session", props: { agentId: "codex", agentLabel: "Codex", status: { exited: 0 } } },
    { name: "inactive member row", props: { id: "inactive-1", agentId: "codex", agentLabel: "Codex", status: { exited: 0 } } },
  ];

  for (const testCase of CASES) {
    it(`renders the same badge markup: ${testCase.name}`, async () => {
      const settings = baseSettings({ agents: TWO_AGENTS, codingAgentProfiles: profiles({}) });
      const rendered = await renderRow(testCase.props, settings);
      try {
        await waitFor(() =>
          expect(rendered.root.querySelector(".ac-discovery-badge.agent")).not.toBeNull(),
        );
        const el = rendered.root.querySelector<HTMLElement>(".ac-discovery-badge.agent")!;
        expect(el.className).toBe("ac-discovery-badge agent");
        expect(el.hasAttribute("data-agent")).toBe(false);
        expect(el.classList.contains("running")).toBe(false);
        expect(el.classList.contains("agent-badge")).toBe(false);
        expect(rendered.root.querySelector(".agent-badge")).toBeNull();
      } finally {
        rendered.cleanup();
      }
    });
  }
});
