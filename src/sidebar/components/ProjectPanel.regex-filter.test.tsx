// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
  session,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import type { AcLoopSummary, AgentConfig } from "../../shared/types";

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;

function projectDiscovery() {
  return discovery({
    agents: [],
    teams: [
      {
        name: "frontend-team",
        agents: ["AgentsCommander_ac/__agent_dev-webpage-ui"],
        coordinator: "AgentsCommander_ac/__agent_dev-webpage-ui",
      },
    ],
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Sidebar regex filter",
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${workgroupPath}\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: "dev-rust",
            path: `${workgroupPath}\\__agent_dev-rust`,
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
    ],
  });
}

function loop(overrides: Partial<AcLoopSummary> = {}): AcLoopSummary {
  return {
    id: "loop-standup",
    name: "Weekday standup",
    enabled: true,
    expr: "0 9 * * 1-5",
    timezone: "local",
    targetKind: "workgroupCoordinator",
    workgroup: "wg-2-dev-team",
    promptPreview: "scheduled run",
    busyCoordinator: "skip",
    path: `${projectPath}\\.ac\\_loop_standup`,
    configPath: `${projectPath}\\.ac\\_loop_standup\\config.toml`,
    lastCheckedAt: null,
    lastDueAt: null,
    lastDeliveredAt: null,
    lastResult: null,
    pendingDueAt: null,
    lastMissedClosedAt: null,
    nextDueAt: null,
    ...overrides,
  };
}

function discoveryWithLoops(loops: AcLoopSummary[]) {
  return { ...projectDiscovery(), loops };
}

function twoWorkgroupDiscovery() {
  return discovery({
    agents: [],
    teams: [],
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Sidebar regex filter",
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${workgroupPath}\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
      {
        name: "wg-3-ops-team",
        path: `${projectPath}\\.ac\\wg-3-ops-team`,
        task: null,
        taskTitle: "Ops rotation",
        agents: [
          {
            name: "ops-lead",
            path: `${projectPath}\\.ac\\wg-3-ops-team\\__agent_ops-lead`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
  });
}

function findByTestId<T extends Element>(root: ParentNode, testId: string): T {
  const element = root.querySelector(`[data-ac-testid="${testId}"]`);
  if (!element) {
    throw new Error(`Element not found: ${testId}`);
  }
  return element as T;
}

/** Minimal valid coding-agent config so a session's `agentId` resolves to a
 *  badge label via settings (the `agent_label: None` runtime path). */
function agentConfig(
  fields: Pick<AgentConfig, "id" | "label" | "command">
): AgentConfig {
  return {
    color: "#888888",
    envs: [],
    isolatedHome: false,
    ...fields,
  };
}

describe("ProjectPanel regex filter", () => {
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

  it("opens inline, filters presentation, reports invalid regex, and clears with Escape", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.resolve("discover_project", projectDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));
      expect(rendered.root.textContent).toContain("dev-rust");

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);

      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");
      await waitFor(() => expect(toggle.getAttribute("aria-expanded")).toBe("true"));
      await waitFor(() => expect(document.activeElement).toBe(filterInput));

      input(filterInput, "webpage");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
      });

      input(filterInput, "(");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("Invalid regex");
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("dev-rust");
      });

      filterInput.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      await waitFor(() => {
        expect(filterInput.value).toBe("");
        expect(rendered.root.textContent).not.toContain("Invalid regex");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the regex search controls in the project header and clears them when closed", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.resolve("discover_project", projectDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      const header = rendered.root.querySelector<HTMLElement>(".project-header");
      expect(header).toBeTruthy();

      const title = header!.querySelector<HTMLElement>(".project-title");
      const row = findByTestId<HTMLDivElement>(rendered.root, "project.regexFilter.row");
      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      expect(title?.textContent).toBe("Project: Project");
      expect(header!.contains(row)).toBe(true);
      expect(header!.contains(toggle)).toBe(true);
      expect(header!.contains(filterInput)).toBe(true);

      click(toggle);
      await waitFor(() => expect(toggle.getAttribute("aria-expanded")).toBe("true"));

      input(filterInput, "dev-rust");
      await waitFor(() => expect(filterInput.value).toBe("dev-rust"));

      click(toggle);
      await waitFor(() => {
        expect(toggle.getAttribute("aria-expanded")).toBe("false");
        expect(filterInput.value).toBe("");
        expect(row.classList.contains("open")).toBe(false);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("matches visible team project labels and quick-access running-peer badges", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.resolve("discover_project", projectDiscovery());

    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-2-dev-team/dev-webpage-ui",
        workingDirectory: `${workgroupPath}\\__agent_dev-webpage-ui`,
        status: "running",
      }),
      session({
        id: "peer-session",
        name: "wg-2-dev-team/dev-rust",
        workingDirectory: `${workgroupPath}\\__agent_dev-rust`,
        status: "running",
      }),
    ]);
    sessionsStore.setActiveId("coord-session");

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("frontend-team"));
      const teamHeader = Array.from(rendered.root.querySelectorAll(".ac-team-header")).find((header) =>
        header.textContent?.includes("frontend-team")
      );
      if (!teamHeader) {
        throw new Error("Team header not found");
      }
      click(teamHeader);

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "AgentsCommander_ac");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui@AgentsCommander_ac");
        expect(rendered.root.textContent).toContain("frontend-team");
      });

      input(filterInput, "RUNNING");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("dev-rust RUNNING");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("filters loop rows and treats the Loops section label as matchable", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discoveryWithLoops([
        loop({ id: "loop-standup", name: "Weekday standup", promptPreview: "morning sync" }),
        loop({
          id: "loop-backup",
          name: "Nightly backup",
          expr: "0 2 * * *",
          promptPreview: "archive job",
          path: `${projectPath}\\.ac\\_loop_backup`,
          configPath: `${projectPath}\\.ac\\_loop_backup\\config.toml`,
        }),
      ])
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("Weekday standup"));
      expect(rendered.root.textContent).toContain("Nightly backup");

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "standup");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("Weekday standup");
        expect(rendered.root.textContent).not.toContain("Nightly backup");
      });

      // The section label itself is matchable -> every loop shows again.
      input(filterInput, "Loops");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("Weekday standup");
        expect(rendered.root.textContent).toContain("Nightly backup");
      });

      // No match anywhere -> loop rows hide cleanly with no empty-state hint.
      input(filterInput, "zzz-no-such-loop");
      await waitFor(() => {
        expect(rendered.root.textContent).not.toContain("Weekday standup");
        expect(rendered.root.textContent).not.toContain("Nightly backup");
        expect(rendered.root.textContent).not.toContain("No loops");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("updates the Workgroups section count to reflect filtered matches", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", twoWorkgroupDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("wg-3-ops-team"));

      const workgroupsCount = () => {
        const header = Array.from(rendered.root.querySelectorAll(".ac-wg-header")).find(
          (h) => h.querySelector(".ac-wg-name")?.textContent === "Workgroups"
        );
        return header?.querySelector(".ac-team-count")?.textContent ?? null;
      };

      await waitFor(() => expect(workgroupsCount()).toBe("2"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "ops-team");
      await waitFor(() => {
        expect(workgroupsCount()).toBe("1");
        expect(rendered.root.textContent).toContain("wg-3-ops-team");
        expect(rendered.root.textContent).not.toContain("wg-2-dev-team");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("closes and clears the active filter when the magnifier is toggled", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");
      await waitFor(() => expect(toggle.getAttribute("aria-expanded")).toBe("true"));

      input(filterInput, "webpage");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
      });

      // Toggling the magnifier closes the field AND drops the filter so no
      // active-but-hidden filter survives.
      click(toggle);
      await waitFor(() => {
        expect(toggle.getAttribute("aria-expanded")).toBe("false");
        expect(filterInput.value).toBe("");
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("dev-rust");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("is presentation-only — an active filter never mutates project/session data", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discoveryWithLoops([loop({ id: "loop-standup", name: "Weekday standup" })])
    );

    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-2-dev-team/dev-webpage-ui",
        workingDirectory: `${workgroupPath}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
      }),
      session({
        id: "peer-session",
        name: "wg-2-dev-team/dev-rust",
        workingDirectory: `${workgroupPath}\\__agent_dev-rust`,
        // Exited (not a running peer) so the matched coordinator does not render
        // a legitimate "dev-rust RUNNING" peer badge — keeps the hide assertion
        // about the dev-rust *row*, not data integrity.
        status: { exited: 0 },
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-rust"));

      const wgAgents = () => projectStore.projects[0].workgroups[0].agents.map((a) => a.name);
      const sessionIds = () => sessionsStore.sessions.map((s) => s.id).sort();
      const beforeAgents = wgAgents();
      const beforeSessions = sessionIds();
      const beforeLoops = projectStore.projects[0].loops.map((l) => l.id);
      const beforeTeams = projectStore.projects[0].teams.map((t) => t.name);

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // A filter that hides the non-matching rows.
      input(filterInput, "webpage");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
      });

      // The store-backed source data is untouched — only DOM visibility changed.
      expect(wgAgents()).toEqual(beforeAgents);
      expect(sessionIds()).toEqual(beforeSessions);
      expect(projectStore.projects[0].loops.map((l) => l.id)).toEqual(beforeLoops);
      expect(projectStore.projects[0].teams.map((t) => t.name)).toEqual(beforeTeams);

      // Clearing the filter restores the hidden row from the same untouched data.
      input(filterInput, "");
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-rust"));
      expect(wgAgents()).toEqual(beforeAgents);
    } finally {
      rendered.cleanup();
    }
  });

  it("does not surface a non-coordinator row by its hidden branch, yet a coordinator's shown branch still matches (bug 1)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());

    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-2-dev-team/dev-webpage-ui",
        workingDirectory: `${workgroupPath}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
        gitRepos: [{ label: "uirepo", sourcePath: "C:\\repo-ui", branch: "coordbranch" }],
      }),
      session({
        id: "peer-session",
        name: "wg-2-dev-team/dev-rust",
        workingDirectory: `${workgroupPath}\\__agent_dev-rust`,
        status: "running",
        isCoordinator: false,
        gitRepos: [{ label: "rustrepo", sourcePath: "C:\\repo-rust", branch: "peerbranch" }],
      }),
    ]);
    sessionsStore.setActiveId("coord-session");

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-rust"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // The non-coordinator's branch badge is never rendered, so its branch is
      // not matchable — dev-rust must NOT be surfaced (the bug).
      input(filterInput, "peerbranch");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("dev-rust"));

      // dev-rust still matches by a field that IS visible (its name).
      input(filterInput, "dev-rust");
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-rust"));

      // The coordinator's branch badge IS rendered, so its branch stays matchable.
      input(filterInput, "coordbranch");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("uirepo/coordbranch");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("does not match a session's hidden shell string (bug 1)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());

    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-2-dev-team/dev-webpage-ui",
        workingDirectory: `${workgroupPath}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
        shell: "bash",
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // `shell` is never rendered on a row, so filtering by it surfaces nothing.
      input(filterInput, "bash");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("dev-webpage-ui"));

      // The same row still matches by its visible name.
      input(filterInput, "webpage");
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));
    } finally {
      rendered.cleanup();
    }
  });

  it("does not match an exited replica by stale waiting/pending flags (#689)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());

    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-2-dev-team/dev-webpage-ui",
        workingDirectory: `${workgroupPath}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
      }),
      session({
        id: "peer-session",
        name: "wg-2-dev-team/dev-rust",
        workingDirectory: `${workgroupPath}\\__agent_dev-rust`,
        status: { exited: 0 },
        waitingForInput: true,
        pendingReview: true,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-rust"));
      const staleRow = findByTestId<HTMLElement>(
        rendered.root,
        "replica.row.workgroups.wg-2-dev-team.dev-rust"
      );
      expect(staleRow.querySelector(".session-item-status")?.classList.contains("exited")).toBe(true);

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "waiting");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("dev-rust"));

      input(filterInput, "pending");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("dev-rust"));

      input(filterInput, "exited");
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-rust"));
    } finally {
      rendered.cleanup();
    }
  });

  it("does not match a loop's promptPreview (hover-only), but the loop name still matches (bug 1)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discoveryWithLoops([
        loop({ id: "loop-standup", name: "Weekday standup", promptPreview: "secretpreviewtoken" }),
      ])
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("Weekday standup"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // promptPreview renders only as the row's hover title, so it is not matchable.
      input(filterInput, "secretpreviewtoken");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("Weekday standup"));

      // The loop's visible name still matches.
      input(filterInput, "standup");
      await waitFor(() => expect(rendered.root.textContent).toContain("Weekday standup"));
    } finally {
      rendered.cleanup();
    }
  });

  it("matches a row by its visible profile badge token (bug 2)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());

    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-2-dev-team/dev-webpage-ui",
        workingDirectory: `${workgroupPath}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
        requestedProfile: "A",
        effectiveProfile: "B",
        profileFallbackApplied: true,
      }),
    ]);
    sessionsStore.setActiveId("coord-session");

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      // The A->B badge is rendered on the coordinator row.
      await waitFor(() => expect(rendered.root.textContent).toContain("A->B"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // Filtering by the shown badge keeps the row visible (no "false hide").
      input(filterInput, "A->B");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("A->B");
      });

      // A profile token that is NOT shown hides the row (confirms the match was
      // the badge, not something else).
      input(filterInput, "C->D");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("dev-webpage-ui"));
    } finally {
      rendered.cleanup();
    }
  });

  it("only matches an agent SessionItem's profile badge when that badge is actually shown (bug 1)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discovery({
        agents: [
          { name: "AgentsCommander_ac/__agent_solohidden", path: `${projectPath}\\.ac\\_agent_solohidden`, roleExists: true },
          { name: "AgentsCommander_ac/__agent_soloshown", path: `${projectPath}\\.ac\\_agent_soloshown`, roleExists: true },
        ],
      })
    );

    sessionsStore.setSessions([
      // No agent label resolves and not a coordinator-with-repos → SessionItem's
      // meta block (and thus its profile badge) is NOT rendered.
      session({
        id: "solohidden-session",
        name: "AgentsCommander_ac/__agent_solohidden",
        workingDirectory: `${projectPath}\\.ac\\_agent_solohidden`,
        status: "running",
        isCoordinator: false,
        agentId: null,
        agentLabel: null,
        gitRepos: [],
        requestedProfile: "A",
        effectiveProfile: "B",
        profileFallbackApplied: true,
      }),
      // Coordinator with repos → meta block renders, so its profile badge shows.
      session({
        id: "soloshown-session",
        name: "AgentsCommander_ac/__agent_soloshown",
        workingDirectory: `${projectPath}\\.ac\\_agent_soloshown`,
        status: "running",
        isCoordinator: true,
        agentId: null,
        agentLabel: null,
        gitRepos: [{ label: "shownrepo", sourcePath: "C:\\repo-shown", branch: "x" }],
        requestedProfile: "C",
        effectiveProfile: "D",
        profileFallbackApplied: true,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("solohidden"));
      // The hidden agent's badge is genuinely absent; the shown agent's is present.
      expect(rendered.root.textContent).not.toContain("A->B");
      expect(rendered.root.textContent).toContain("C->D");

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // Hidden badge → not matchable (must not surface the row).
      input(filterInput, "A->B");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("solohidden"));

      // Shown badge → matchable (surfaces only the row that displays it).
      input(filterInput, "C->D");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("soloshown");
        expect(rendered.root.textContent).not.toContain("solohidden");
      });

      // Both still match by their visible names.
      input(filterInput, "solohidden");
      await waitFor(() => expect(rendered.root.textContent).toContain("solohidden"));
    } finally {
      rendered.cleanup();
    }
  });

  it("closes the filter when Escape is pressed on an empty field", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");
      await waitFor(() => expect(toggle.getAttribute("aria-expanded")).toBe("true"));
      expect(filterInput.value).toBe("");

      // Escape on an already-empty field closes the filter outright — the
      // previously untested empty branch of handleFilterKeyDown.
      filterInput.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })
      );
      await waitFor(() => expect(toggle.getAttribute("aria-expanded")).toBe("false"));
    } finally {
      rendered.cleanup();
    }
  });

  it("matches an agent row by its coding-agent badge resolved from agentId, per row (#515)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discovery({
        agents: [
          { name: "AgentsCommander_ac/__agent_alpha", path: `${projectPath}\\.ac\\_agent_alpha`, roleExists: true },
          { name: "AgentsCommander_ac/__agent_bravo", path: `${projectPath}\\.ac\\_agent_bravo`, roleExists: true },
        ],
      })
    );
    // The badge label is resolved from agentId via settings (agent_label: None).
    fake.resolve(
      "get_settings",
      baseSettings({
        agents: [
          agentConfig({ id: "claude", label: "Claude Code", command: "claude" }),
          agentConfig({ id: "opencode", label: "opencode", command: "opencode" }),
        ],
      })
    );

    sessionsStore.setSessions([
      session({
        id: "alpha-session",
        name: "AgentsCommander_ac/__agent_alpha",
        workingDirectory: `${projectPath}\\.ac\\_agent_alpha`,
        status: "running",
        agentId: "claude",
        agentLabel: null,
      }),
      session({
        id: "bravo-session",
        name: "AgentsCommander_ac/__agent_bravo",
        workingDirectory: `${projectPath}\\.ac\\_agent_bravo`,
        status: "running",
        agentId: "opencode",
        agentLabel: null,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("alpha"));
      // Each row renders its resolved coding-agent badge.
      expect(rendered.root.textContent).toContain("Claude Code");
      expect(rendered.root.textContent).toContain("opencode");

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // The coding-agent badge is now matchable — and only on the row that shows
      // it (the name "alpha" never contains "Claude Code", so the match is the badge).
      input(filterInput, "Claude Code");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("alpha");
        expect(rendered.root.textContent).not.toContain("bravo");
      });

      input(filterInput, "opencode");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("bravo");
        expect(rendered.root.textContent).not.toContain("alpha");
      });

      // Both still match by their visible names.
      input(filterInput, "a"); // matches alpha and bravo
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("alpha");
        expect(rendered.root.textContent).toContain("bravo");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("matches an agent row by its visible profile letter when the meta badge is shown (#515)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discovery({
        agents: [
          { name: "AgentsCommander_ac/__agent_charlie", path: `${projectPath}\\.ac\\_agent_charlie`, roleExists: true },
        ],
      })
    );
    fake.resolve(
      "get_settings",
      baseSettings({ agents: [agentConfig({ id: "claude", label: "Claude Code", command: "claude" })] })
    );

    sessionsStore.setSessions([
      // A resolvable agentId renders the meta block, so the profile letter inside
      // it is shown — and therefore must be matchable (the bug-2 gate, verified
      // here for the resolved-label path the user hit).
      session({
        id: "charlie-session",
        name: "AgentsCommander_ac/__agent_charlie",
        workingDirectory: `${projectPath}\\.ac\\_agent_charlie`,
        status: "running",
        agentId: "claude",
        agentLabel: null,
        requestedProfile: "Z",
        effectiveProfile: "Z",
        profileFallbackApplied: false,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("charlie"));
      expect(rendered.root.textContent).toContain("Z"); // profile badge shown

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // The shown profile letter matches the row.
      input(filterInput, "Z");
      await waitFor(() => expect(rendered.root.textContent).toContain("charlie"));

      // A profile letter that is NOT shown hides the row — confirms the match was
      // the profile badge, not the name or the coding-agent label.
      input(filterInput, "Q");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("charlie"));
    } finally {
      rendered.cleanup();
    }
  });

  it("matches a coordinator quick-access row by its visible task title (#515)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discovery({
        workgroups: [
          {
            name: "wg-2-dev-team",
            path: workgroupPath,
            task: null,
            taskTitle: "Quasar telemetry rollout",
            agents: [
              {
                name: "dev-webpage-ui",
                path: `${workgroupPath}\\__agent_dev-webpage-ui`,
                repoPaths: [],
                isCoordinator: true,
              },
            ],
          },
        ],
      })
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("Quasar telemetry rollout"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // "telemetry" appears only in the task title, yet the coordinator row that
      // displays it stays visible — the task title is matchable.
      input(filterInput, "telemetry");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("Quasar telemetry rollout");
      });

      // A token shown nowhere hides the row.
      input(filterInput, "nonsensetoken");
      await waitFor(() => expect(rendered.root.textContent).not.toContain("dev-webpage-ui"));
    } finally {
      rendered.cleanup();
    }
  });

  it("matches a shut-down replica by its persisted Coding Agent label and Profile letter (#733/#515)", async () => {
    // #733 makes a gray replica show its persisted Coding Agent + Profile badges;
    // #515 requires the filter to match what is visible. Both rows are dormant
    // members (no session): dev-webpage-ui carries a persisted codex/Z, dev-rust
    // carries nothing. Neither "codex" nor "z" appears anywhere in dev-webpage-ui's
    // row text except its badges, so a match proves the persisted label + letter
    // now flow into replicaSearchText (before the fix they did not -> the row would
    // be filtered out while visibly showing a Codex badge).
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "get_settings",
      baseSettings({ agents: [agentConfig({ id: "codex", label: "Codex", command: "codex" })] })
    );
    fake.resolve(
      "discover_project",
      discovery({
        agents: [],
        teams: [],
        workgroups: [
          {
            name: "wg-2-dev-team",
            path: workgroupPath,
            task: null,
            taskTitle: "Sidebar regex filter",
            agents: [
              {
                name: "dev-webpage-ui",
                path: `${workgroupPath}\\__agent_dev-webpage-ui`,
                repoPaths: [],
                isCoordinator: false,
                currentCodingAgentId: "codex",
                currentProfile: "Z",
              },
              {
                name: "dev-rust",
                path: `${workgroupPath}\\__agent_dev-rust`,
                repoPaths: [],
                isCoordinator: false,
              },
            ],
          },
        ],
      })
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));
      // Both dormant rows are visible unfiltered, and the persisted badge shows.
      expect(rendered.root.textContent).toContain("dev-rust");
      expect(rendered.root.textContent).toContain("Codex");

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      // (a) filter by the persisted Coding Agent label — matchable only via the badge.
      input(filterInput, "codex");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
      });

      // (b) filter by the persisted Profile letter — "z" is nowhere in the row text
      // except the persisted profile badge.
      input(filterInput, "z");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
      });
    } finally {
      rendered.cleanup();
    }
  });
});
