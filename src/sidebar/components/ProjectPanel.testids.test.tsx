// @vitest-environment jsdom
// #4825 - every new sidebar menu data-ac-testid in ProjectPanel resolves to
// exactly one element, and the replica TASK title editor ids drive the editor.
// Two Telegram bots are configured so Attach opens the bot choice list.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type { AcDiscoveryResult, Session } from "../../shared/types";
import { automationIdPart as AP } from "./replica-repo-badges";

const projectPath = "C:\\Project";
const otherProjectPath = "D:\\OtherProject";
const WG_NAME = "wg-2-dev-team";
const wgPathOf = (proj: string) => `${proj}\\.ac\\${WG_NAME}`;
const coordPathOf = (proj: string) => `${wgPathOf(proj)}\\__agent_dev-webpage-ui`;
const memberPathOf = (proj: string) => `${wgPathOf(proj)}\\__agent_dev-rust`;
const agentPath = `${projectPath}\\.ac\\_agent_dev-docs`;
const PA = AP(projectPath);
const memberRow = `replica.row.workgroups.${WG_NAME}.dev-rust`;
const coordRow = `replica.row.quick.${WG_NAME}.dev-webpage-ui`;
const BOT = { id: "bot-one", label: "Bot One", color: "red" };
const BOT_2 = { id: "bot-two", label: "Bot Two", color: "blue" };

function projectDiscovery(proj: string): AcDiscoveryResult {
  return discovery({
    agents: [{ name: "dev-docs", path: agentPath, roleExists: true }],
    teams: [{ name: "team-alpha", coordinator: "dev-webpage-ui", agents: ["dev-webpage-ui"] }],
    workgroups: [
      {
        name: WG_NAME,
        path: wgPathOf(proj),
        task: null,
        taskTitle: "Old title",
        agents: [
          {
            name: "dev-webpage-ui",
            path: coordPathOf(proj),
            identityPath: "../../_agent_dev-webpage-ui",
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: "dev-rust",
            path: memberPathOf(proj),
            identityPath: "../../_agent_dev-rust",
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
    ],
  });
}

function memberSession(): Session {
  return session({
    id: "member-session",
    name: `${WG_NAME}/dev-rust`,
    workingDirectory: memberPathOf(projectPath),
    status: "running",
  });
}

function byId<T extends HTMLElement = HTMLElement>(id: string): T {
  const all = document.querySelectorAll<T>(`[data-ac-testid="${id}"]`);
  if (all.length !== 1) throw new Error(`${id}: expected 1 element, found ${all.length}`);
  return all[0];
}

async function expectOne(...ids: string[]): Promise<void> {
  await waitFor(() => {
    for (const id of ids) byId(id);
  });
}

function textButton(label: string): HTMLButtonElement {
  const found = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
    (b) => b.textContent?.trim() === label,
  );
  if (!found) throw new Error(`button not found: ${label}`);
  return found;
}

function headerByName(name: string): HTMLElement {
  const found = Array.from(document.querySelectorAll<HTMLElement>(".ac-wg-header")).find(
    (h) => h.querySelector(".ac-wg-name")?.textContent?.trim() === name,
  );
  if (!found) throw new Error(`header not found: ${name}`);
  return found;
}

async function dismissMenus(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await waitFor(() => expect(document.querySelector(".session-context-menu")).toBeNull());
}

describe("#4825 ProjectPanel sidebar menu test ids", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });
  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  async function setup(sessions: Session[], projects = [projectPath]): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => projectDiscovery(args.path as string));
    fake.resolve("get_settings", { telegramBots: [BOT, BOT_2] });
    fake.resolve("task_get_title", "Old title");
    fake.resolve("task_set_title", { workgroupRoot: wgPathOf(projectPath), task: null });
    fake.resolve("task_set_title_at", { workgroupRoot: wgPathOf(projectPath), task: null });
    fake.resolve("open_in_explorer", null);
    sessionsStore.setSessions(sessions);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    for (const proj of projects) await projectStore.createAndLoad(proj);
    await waitFor(() => expect(rendered!.root.textContent).toContain("dev-rust"));
    return fake;
  }

  it("active replica menu: ids #21-27, Telegram bot choice #5, title editor #6-8", async () => {
    const fake = await setup([memberSession()]);
    const A = "replica.member-session.menu";

    contextMenu(byId(memberRow));
    await expectOne(
      `${A}.restart`,
      `${A}.openFolder`,
      `${A}.detachToggle`,
      `${A}.telegram`,
      `${A}.editTaskTitle`,
      `${A}.clearTaskTitle`,
    );
    await expectOne(`${A}.matrixFolder`);

    click(byId(`${A}.telegram`));
    await expectOne(`${A}.telegram.bot.${BOT.id}`, `${A}.telegram.bot.${BOT_2.id}`);

    click(byId(`${A}.editTaskTitle`));
    await expectOne(`${A}.editTaskTitle.input`, `${A}.editTaskTitle.save`);
    const field = byId<HTMLInputElement>(`${A}.editTaskTitle.input`);
    input(field, "I4825 active");
    click(byId(`${A}.editTaskTitle.save`));
    await waitFor(() => expect(fake.lastCall("task_set_title")?.args.title).toBe("I4825 active"));
  });

  it("inactive replica menu: ids #28-31 and title editor Cancel", async () => {
    await setup([]);
    const IK = `replica.inactive.${AP(memberPathOf(projectPath))}.menu`;

    contextMenu(byId(memberRow));
    await expectOne(`${IK}.openFolder`, `${IK}.editTaskTitle`, `${IK}.clearTaskTitle`);
    await expectOne(`${IK}.matrixFolder`);

    click(byId(`${IK}.editTaskTitle`));
    await expectOne(`${IK}.editTaskTitle.input`, `${IK}.editTaskTitle.save`);
    click(byId(`${IK}.editTaskTitle.cancel`));
    await waitFor(() =>
      expect(document.querySelector(`[data-ac-testid="${IK}.editTaskTitle.input"]`)).toBeNull(),
    );
  });

  it("two projects with an open inactive replica menu each: every id is unique", async () => {
    await setup([], [projectPath, otherProjectPath]);
    const panels = Array.from(document.querySelectorAll<HTMLElement>(".project-panel"));
    expect(panels).toHaveLength(2);
    for (const panel of panels) {
      const row = panel.querySelector<HTMLElement>(`[data-ac-testid="${coordRow}"]`);
      if (!row) throw new Error("coordinator row not found");
      contextMenu(row);
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    await waitFor(() =>
      expect(document.querySelectorAll(".session-context-menu").length).toBeGreaterThanOrEqual(2),
    );
    for (const proj of [projectPath, otherProjectPath]) {
      const IK = `replica.inactive.${AP(coordPathOf(proj))}.menu`;
      await expectOne(`${IK}.openFolder`, `${IK}.editTaskTitle`, `${IK}.clearTaskTitle`);
    }
  });

  it("project header toggle and project menu: #10-13", async () => {
    await setup([]);
    await expectOne(`project.header.${PA}.toggle`);
    contextMenu(document.querySelector<HTMLElement>(".project-header")!);
    await expectOne(
      `project.action.newAgent.${PA}.projectMenu`,
      `project.action.newTeam.${PA}.projectMenu`,
      `project.action.remove.${PA}`,
    );
  });

  it("rooms, agents and teams headers: #14, #16, #17", async () => {
    await setup([]);
    contextMenu(headerByName("Rooms"));
    await expectOne(`project.action.newRoom.${PA}.roomsHeader`);
    await dismissMenus();
    contextMenu(headerByName("Agents"));
    await expectOne(`project.action.newAgent.${PA}.agentsHeader`);
    await dismissMenus();
    contextMenu(headerByName("Teams"));
    await expectOne(`project.action.newTeam.${PA}.teamsHeader`);
  });

  it("agent menu #15, team menu #18-19 and delete-team dialog #36-37", async () => {
    await setup([]);
    const agentRow = Array.from(document.querySelectorAll<HTMLElement>(".replica-item")).find(
      (row) => row.getAttribute("title") === agentPath,
    );
    if (!agentRow) throw new Error("agent row not found");
    contextMenu(agentRow);
    await expectOne(`agent.action.matrixFolder.${AP(agentPath)}`);
    await dismissMenus();

    const team = `${PA}.${AP("team-alpha")}`;
    contextMenu(document.querySelector<HTMLElement>(".ac-team-header")!);
    await expectOne(`team.action.edit.${team}`, `team.action.delete.${team}`);
    click(byId(`team.action.delete.${team}`));
    await expectOne(`team.delete.cancel.${team}`, `team.delete.confirm.${team}`);
  });

  it("room menu #20 and delete-room dialog #32, #34-35", async () => {
    const fake = await setup([]);
    fake.onInvoke("delete_workgroup", () => {
      throw "DIRTY_REPOS:repo has uncommitted changes";
    });
    const wgKey = AP(wgPathOf(projectPath));
    const wgHeader = Array.from(document.querySelectorAll<HTMLElement>(".ac-wg-header")).find(
      (h) => h.getAttribute("title") === wgPathOf(projectPath),
    );
    if (!wgHeader) throw new Error("room header not found");
    contextMenu(wgHeader);
    await expectOne(`workgroup.action.delete.${wgKey}`);
    click(byId(`workgroup.action.delete.${wgKey}`));
    await expectOne(`workgroup.delete.cancel.${wgKey}`, `workgroup.delete.confirm.${wgKey}`);

    click(byId(`workgroup.delete.confirm.${wgKey}`));
    await waitFor(() => expect(document.body.textContent).toContain("repo has uncommitted"));
    await expectOne(`workgroup.delete.confirmInput.${wgKey}`);
    expect(textButton("Cancel")).toBe(byId(`workgroup.delete.cancel.${wgKey}`));
  });
});
