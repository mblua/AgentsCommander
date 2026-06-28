// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
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
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";

const projectPath = "C:\\Project";
const teamName = "dev-team";
const agentPath = `${projectPath}\\.ac\\_agent_dev-webpage-ui`;
const unsavedRepoUrl = "https://github.com/acme/unsaved-repo.git";

function setupTransport(fake: FakeTransport): void {
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve(
    "discover_project",
    discovery({
      teams: [
        {
          name: teamName,
          agents: ["dev-webpage-ui"],
          coordinator: "dev-webpage-ui",
        },
      ],
    }),
  );
  fake.resolve("list_all_agents", [
    {
      name: "dev-webpage-ui",
      path: agentPath,
      projectName: "Project",
    },
  ]);
  fake.resolve("get_team_config", {
    agents: ["_agent_dev-webpage-ui"],
    coordinator: "_agent_dev-webpage-ui",
    repos: [],
  });
  fake.resolve("update_team", undefined);
}

function findTeamHeader(root: ParentNode): HTMLElement {
  const header = Array.from(root.querySelectorAll<HTMLElement>(".ac-team-header")).find(
    (el) => el.textContent?.includes(teamName),
  );
  if (!header) throw new Error(`Team header not found: ${teamName}`);
  return header;
}

function findButtonByText(label: string): HTMLButtonElement {
  const button = Array.from(document.body.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!(button instanceof HTMLButtonElement)) throw new Error(`Button not found: ${label}`);
  return button;
}

function repoInput(): HTMLInputElement {
  const el = document.body.querySelector<HTMLInputElement>(
    'input[placeholder="https://github.com/org/repo.git"]',
  );
  if (!el) throw new Error("Repo input not found");
  return el;
}

function modalText(): string {
  return document.body.textContent ?? "";
}

describe("ProjectPanel edit-team modal (#669)", () => {
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
  });

  it("keeps the edit team modal and unsaved repo edits open across project refreshes", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);

    await waitFor(() => expect(findTeamHeader(rendered!.root)).toBeTruthy());
    contextMenu(findTeamHeader(rendered.root));
    await waitFor(() => expect(findButtonByText("Edit Team")).toBeTruthy());
    click(findButtonByText("Edit Team"));

    await waitFor(() => expect(modalText()).toContain(`Edit Team: ${teamName}`));
    await waitFor(() => expect(findButtonByText("Next").disabled).toBe(false));
    click(findButtonByText("Next"));
    await waitFor(() => expect(findButtonByText("Next").disabled).toBe(false));
    click(findButtonByText("Next"));

    await waitFor(() => expect(repoInput()).toBeTruthy());
    input(repoInput(), unsavedRepoUrl);
    click(findButtonByText("Add Repo"));
    await waitFor(() => expect(modalText()).toContain("unsaved-repo"));

    await projectStore.reloadProject(projectPath);
    await waitFor(() =>
      expect(fake.callsFor("discover_project").length).toBeGreaterThanOrEqual(2),
    );

    expect(modalText()).toContain(`Edit Team: ${teamName}`);
    expect(modalText()).toContain("unsaved-repo");
  });
});
