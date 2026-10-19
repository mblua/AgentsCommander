// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NewTeamModal from "./NewTeamModal";
import { projectStore } from "../stores/project";
import type { AcDiscoveryResult } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, discovery, input, installBrowserDomStubs, renderWithFakeTransport,
  resetUiStoresForTests, waitFor } from "../../shared/testing/ui-harness";

const destination = "C:/Destination";
const local = { name: "Copied role", path: `${destination}/.ac/_agent_local`,
  projectName: "Destination", projectPath: destination, isLocal: true,
  provenance: { copies: [{ sourceProjectName: "Historical" }] } };
const foreign = { name: "Visible role", path: "D:/one/.ac/_agent_remote",
  projectName: "Shared", projectPath: "D:/one", isLocal: false };
const other = { name: "Other role", path: "E:/two/.ac/_agent_other",
  projectName: "Shared", projectPath: "E:/two", isLocal: false };

describe("NewTeamModal cross-project identity", () => {
  let rendered: ReturnType<typeof renderWithFakeTransport> | undefined;
  let restoreDom: () => void;
  let fake: FakeTransport;
  let close: ReturnType<typeof vi.fn<() => void>>;
  beforeEach(() => {
    restoreDom = installBrowserDomStubs();
    resetUiStoresForTests();
    fake = new FakeTransport();
    fake.resolve("create_team", undefined);
    fake.resolve("discover_project", discovery());
    close = vi.fn();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });
  afterEach(() => {
    rendered?.cleanup();
    restoreDom();
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });
  function button(text: string): HTMLButtonElement {
    const found = Array.from(document.querySelectorAll("button")).find(b => b.textContent?.trim() === text);
    if (!found) throw new Error(`Missing button ${text}`);
    return found;
  }
  function field(selector: string): HTMLInputElement {
    const found = document.querySelector<HTMLInputElement>(selector);
    if (!found) throw new Error(`Missing input ${selector}`);
    return found;
  }
  async function start(agents: unknown[]) {
    fake.resolve("list_all_agents", agents);
    rendered = renderWithFakeTransport(() => <NewTeamModal projectPath={destination} onClose={close} />, fake);
    input(field('[placeholder="dream-team"]'), "team");
    click(button("Next"));
    await waitFor(() => expect(document.querySelector(".wizard-loading")).toBeNull());
    expect(fake.lastCall("list_all_agents")?.args.destinationProjectPath).toBe(destination);
  }
  function selectAll(coordinatorIndex = 0) {
    document.querySelectorAll('.wizard-agent-row input[type="checkbox"]').forEach(click);
    click(document.querySelectorAll('input[type="radio"]')[coordinatorIndex]);
    click(button("Next"));
  }
  function addRepo() {
    input(field('[placeholder="https://github.com/org/repo.git"]'), "https://example.test/repo.git");
    click(button("Add Repo"));
  }

  it("keeps backend identity, labels, coordinator, repositories and alerts through creation", async () => {
    const order: string[] = [];
    fake.onInvoke("create_team", () => { order.push("create"); });
    fake.onInvoke("discover_project", () => { order.push("reload"); return discovery(); });
    close.mockImplementation(() => order.push("close"));
    await start([local, foreign, other]);
    const names = Array.from(document.querySelectorAll(".wizard-agent-name"));
    expect(names.map(n => n.textContent)).toEqual(["Copied role", "Visible role@Shared", "Other role@Shared"]);
    expect(names[0].querySelector(".wizard-agent-project")).toBeNull();
    expect(names[1].querySelector(".wizard-agent-project")?.textContent).toBe("@Shared");
    selectAll(1);
    addRepo();
    click(document.querySelectorAll('.wizard-repo-agents input[type="checkbox"]')[3]);
    expect(Array.from(document.querySelectorAll(".wizard-repo-agents .wizard-agent-project"))
      .map(n => n.textContent)).toEqual(["@Shared", "@Shared"]);
    click(button("Add threshold"));
    input(field(".team-context-alert-input"), "75");
    click(button("Create"));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(fake.lastCall("create_team")?.args).toEqual({
      projectPath: destination, name: "team", agents: ["_agent_local", "_agent_remote", "_agent_other"],
      coordinator: "_agent_remote",
      repos: [{ url: "https://example.test/repo.git", agents: ["_agent_local", "_agent_remote"] }],
      contextAlertPercentages: [75],
      agentSources: [
        { agentRef: "_agent_local", sourceProjectPath: destination },
        { agentRef: "_agent_remote", sourceProjectPath: "D:/one" },
        { agentRef: "_agent_other", sourceProjectPath: "E:/two" },
      ],
    });
    expect(order).toEqual(["create", "reload", "close"]);
  });

  it("rejects portable homonyms without losing selection and permits correction", async () => {
    await start([foreign, { ...other, path: "E:/two/.ac/_agent_remote" }]);
    selectAll();
    click(button("Create"));
    expect(document.querySelector('[aria-label="Team creation error"]')?.textContent).toContain("same portable reference");
    expect(fake.callsFor("create_team")).toHaveLength(0);
    click(button("Back"));
    const boxes = document.querySelectorAll<HTMLInputElement>('.wizard-agent-row input[type="checkbox"]');
    expect(Array.from(boxes).map(b => b.checked)).toEqual([true, true]);
    expect(field('input[type="radio"]').checked).toBe(true);
    click(boxes[1]);
    click(button("Next"));
    click(button("Create"));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(fake.lastCall("create_team")?.args.agentSources).toEqual([
      { agentRef: "_agent_remote", sourceProjectPath: "D:/one" },
    ]);
  });

  it("uses backend locality for an alias and a rediscovered copy, including local-only creation", async () => {
    await start([{ ...local, projectName: "Alias name", projectPath: "C:/canonical-destination" }]);
    expect(document.querySelector(".wizard-agent-name")?.textContent).toBe("Copied role");
    expect(document.querySelector(".wizard-agent-project")).toBeNull();
    selectAll();
    click(button("Create"));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(fake.lastCall("create_team")?.args.agentSources).toEqual([
      { agentRef: "_agent_local", sourceProjectPath: "C:/canonical-destination" },
    ]);
  });

  it("retains state and shows create failure without reloading or closing", async () => {
    fake.reject("create_team", "request rejected");
    await start([foreign]);
    selectAll();
    addRepo();
    click(button("Create"));
    await waitFor(() => expect(document.querySelector('[aria-label="Team creation error"]')?.textContent)
      .toContain("request rejected"));
    expect(close).not.toHaveBeenCalled();
    expect(fake.callsFor("discover_project")).toHaveLength(0);
    expect(button("Create").disabled).toBe(false);
    expect(document.querySelector(".wizard-repo-agents")?.textContent).toContain("Visible role@Shared");
    click(button("Back"));
    expect(field('input[type="checkbox"]').checked).toBe(true);
    expect(field('input[type="radio"]').checked).toBe(true);
  });

  it("waits for real discovery and preserves seeded state on best-effort failure", async () => {
    const seed = discovery({
      agents: [{ name: "Existing", path: destination + "/.ac/_agent_existing", roleExists: true }],
      teams: [{ name: "existing-team", agents: ["_agent_existing"], coordinator: "_agent_existing" }],
    });
    fake.resolve("open_project", { path: destination, registered: true, created: false });
    fake.resolve("discover_project", seed);
    await start([foreign]);
    await projectStore.loadProject(destination);
    expect(projectStore.projects).toHaveLength(1);
    expect(projectStore.projects[0].agents).toEqual(seed.agents);
    expect(projectStore.projects[0].teams).toEqual(seed.teams);
    const before = structuredClone(projectStore.projects);
    fake.clearCalls(); // Initial discovery belongs to the seed, not this creation.

    let rejectDiscovery!: (reason: unknown) => void;
    const pending = new Promise<AcDiscoveryResult>((_resolve, reject) => { rejectDiscovery = reject; });
    void pending.catch(() => undefined); // Also safe if an earlier assertion prevents consumption.
    fake.onInvoke("discover_project", () => pending);
    try {
      selectAll();
      click(button("Create"));
      await waitFor(() => expect(fake.callsFor("discover_project")).toHaveLength(1));
      expect(fake.callsFor("create_team")).toHaveLength(1);
      expect(fake.lastCall("discover_project")?.args).toEqual({ path: destination });
      expect(close).not.toHaveBeenCalled();
      expect(document.querySelector(".entity-wizard-modal")?.getAttribute("aria-busy")).toBe("true");
      expect(button("Creating...").disabled).toBe(true);
      expect(projectStore.projects).toEqual(before);

      // Existing best-effort reload may leave a stale view. Fake create proves no disk publication.
      rejectDiscovery(new Error("discovery rejected"));
      await waitFor(() => expect(close).toHaveBeenCalledOnce());
      expect(document.querySelector('[aria-label="Team creation error"]')).toBeNull();
      expect(projectStore.projects).toEqual(before);
      expect(fake.callsFor("create_team")).toHaveLength(1);
      expect(fake.callsFor("discover_project")).toHaveLength(1);
    } finally {
      rejectDiscovery(new Error("settle discovery before cleanup"));
      // clear() is not cancellation: drain the real consumer before transport/store teardown.
      await waitFor(() => expect(document.querySelector(".entity-wizard-modal")?.getAttribute("aria-busy"))
        .toBe("false"));
    }
  });

  it.each([
    { ...foreign, projectPath: undefined },
    { ...foreign, isLocal: null },
  ])("shows invalid backend identity instead of inferring it", async (agent) => {
    await start([agent]);
    expect(document.querySelector('[aria-label="Agent selection error"]')?.textContent).toContain("identity is missing");
    expect(document.querySelectorAll(".wizard-agent-row")).toHaveLength(0);
    expect(button("Next").disabled).toBe(true);
  });
});
