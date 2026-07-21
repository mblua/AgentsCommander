// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NewTeamModal from "./NewTeamModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";

const projectPath = "C:\\Project";
const agentPath = `${projectPath}\\.ac\\_agent_dev-webpage-ui`;
const repoUrl = "https://github.com/acme/repo.git";

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolvePromise!: (value: T) => void;
  let rejectPromise!: (reason: unknown) => void;
  const promise = new Promise<T>((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: resolvePromise, reject: rejectPromise };
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve("list_all_agents", [
    { name: "dev-webpage-ui", path: agentPath, projectName: "Project" },
  ]);
  fake.resolve("create_team", undefined);
  fake.resolve("discover_project", discovery());
}

function button(label: string): HTMLButtonElement {
  const found = Array.from(document.body.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!(found instanceof HTMLButtonElement)) throw new Error(`Button not found: ${label}`);
  return found;
}

function field(selector: string): HTMLInputElement {
  const found = document.body.querySelector(selector);
  if (!(found instanceof HTMLInputElement)) throw new Error(`Input not found: ${selector}`);
  return found;
}

async function advanceToStepThree(): Promise<void> {
  await waitFor(() => expect(field('input[placeholder="dream-team"]')).toBeTruthy());
  input(field('input[placeholder="dream-team"]'), "dev-team");
  click(button("Next"));

  await waitFor(() => expect(document.body.querySelector('input[type="checkbox"]')).toBeTruthy());
  const agentCheckbox = document.body.querySelector('input[type="checkbox"]');
  if (!(agentCheckbox instanceof HTMLInputElement)) throw new Error("Agent checkbox missing");
  click(agentCheckbox);
  await waitFor(() => expect(document.body.querySelector('input[type="radio"]')).toBeTruthy());
  const coordinatorRadio = document.body.querySelector('input[type="radio"]');
  if (!(coordinatorRadio instanceof HTMLInputElement)) throw new Error("Coordinator radio missing");
  click(coordinatorRadio);
  await waitFor(() => expect(button("Next").disabled).toBe(false));
  click(button("Next"));
  await waitFor(() => expect(button("Add threshold")).toBeTruthy());
}

function addThreshold(raw: string): HTMLInputElement {
  click(button("Add threshold"));
  const allInputs = Array.from(
    document.body.querySelectorAll<HTMLInputElement>(".team-context-alert-input"),
  );
  const thresholdInput = allInputs[allInputs.length - 1];
  if (!thresholdInput) throw new Error("Threshold input missing");
  input(thresholdInput, raw);
  return thresholdInput;
}

describe("NewTeamModal context alerts", () => {
  let restoreDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(() => {
    restoreDom = installBrowserDomStubs();
    resetUiStoresForTests();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    restoreDom?.();
    restoreDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("submits the default empty policy, reloads, and closes in order", async () => {
    const fake = new FakeTransport();
    const order: string[] = [];
    setupTransport(fake);
    fake.onInvoke("create_team", () => {
      order.push("create");
    });
    fake.onInvoke("discover_project", () => {
      order.push("reload");
      return discovery();
    });
    const onClose = vi.fn(() => order.push("close"));
    rendered = renderWithFakeTransport(
      () => <NewTeamModal projectPath={projectPath} onClose={onClose} />,
      fake,
    );

    await advanceToStepThree();
    click(button("Create"));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));

    expect(fake.callsFor("create_team")).toEqual([
      {
        cmd: "create_team",
        args: {
          projectPath,
          name: "dev-team",
          agents: ["_agent_dev-webpage-ui"],
          coordinator: "_agent_dev-webpage-ui",
          repos: [],
          contextAlertPercentages: [],
        },
      },
    ]);
    expect(order).toEqual(["create", "reload", "close"]);
  });

  it("submits one, two, and three rows canonically without reordering visible drafts", async () => {
    const thresholdCases = [
      { raw: ["50"], expected: [50] },
      { raw: ["75", "25"], expected: [25, 75] },
      { raw: ["90", "50", "75"], expected: [50, 75, 90] },
    ];

    for (const thresholdCase of thresholdCases) {
      const fake = new FakeTransport();
      setupTransport(fake);
      const onClose = vi.fn();
      rendered = renderWithFakeTransport(
        () => <NewTeamModal projectPath={projectPath} onClose={onClose} />,
        fake,
      );
      await advanceToStepThree();
      for (const raw of thresholdCase.raw) addThreshold(raw);
      expect(Array.from(document.body.querySelectorAll<HTMLInputElement>(
        ".team-context-alert-input",
      )).map((element) => element.value)).toEqual(thresholdCase.raw);

      click(button("Create"));
      await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
      expect(fake.lastCall("create_team")?.args.contextAlertPercentages).toEqual(
        thresholdCase.expected,
      );

      rendered.cleanup();
      rendered = null;
      document.body.replaceChildren();
      resetUiStoresForTests();
    }
  });

  it("blocks blank and duplicate drafts, then enables Create after correction", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    rendered = renderWithFakeTransport(
      () => <NewTeamModal projectPath={projectPath} onClose={() => undefined} />,
      fake,
    );
    await advanceToStepThree();

    click(button("Add threshold"));
    const first = field(".team-context-alert-input");
    expect(button("Create").disabled).toBe(true);
    click(button("Create"));
    expect(fake.callsFor("create_team")).toHaveLength(0);

    input(first, "80");
    const second = addThreshold("080");
    expect(first.getAttribute("aria-invalid")).toBe("true");
    expect(second.getAttribute("aria-invalid")).toBe("true");
    expect(button("Create").disabled).toBe(true);
    click(button("Create"));
    expect(fake.callsFor("create_team")).toHaveLength(0);
    expect(fake.callsFor("discover_project")).toHaveLength(0);

    input(second, "90");
    expect(button("Create").disabled).toBe(false);
  });

  it("keeps raw threshold text through Back and Next", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    rendered = renderWithFakeTransport(
      () => <NewTeamModal projectPath={projectPath} onClose={() => undefined} />,
      fake,
    );
    await advanceToStepThree();
    addThreshold("080");

    click(button("Back"));
    await waitFor(() => expect(button("Next")).toBeTruthy());
    click(button("Next"));
    await waitFor(() => expect(field(".team-context-alert-input").value).toBe("080"));
    expect(fake.callsFor("create_team")).toHaveLength(0);
  });

  it("keeps creation, repository, member, and raw state after rejection without conflating errors", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.reject("create_team", "create rejected");
    const onClose = vi.fn();
    rendered = renderWithFakeTransport(
      () => <NewTeamModal projectPath={projectPath} onClose={onClose} />,
      fake,
    );
    await advanceToStepThree();
    const thresholdInput = addThreshold("80");
    const repositoryInput = field('input[placeholder="https://github.com/org/repo.git"]');
    input(repositoryInput, repoUrl);
    click(button("Add Repo"));
    input(repositoryInput, repoUrl);
    click(button("Add Repo"));
    expect(document.body.querySelector('[role="alert"][aria-label="Repository error"]'))
      .toBeTruthy();

    click(button("Create"));
    await waitFor(() => expect(
      document.body.querySelector('[role="alert"][aria-label="Team creation error"]')?.textContent,
    ).toContain("create rejected"));
    expect(document.body.textContent).toContain("repo");
    expect(thresholdInput.value).toBe("80");
    expect(fake.lastCall("create_team")?.args.agents).toEqual(["_agent_dev-webpage-ui"]);
    expect(fake.callsFor("discover_project")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();

    input(thresholdInput, "81");
    expect(document.body.querySelector('[role="alert"][aria-label="Team creation error"]')?.textContent)
      .toContain("create rejected");
    expect(document.body.querySelector('[role="alert"][aria-label="Repository error"]'))
      .toBeTruthy();
  });

  it("locks all step-three interactions around one immutable deferred request", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const pending = deferred<void>();
    fake.onInvoke("create_team", () => pending.promise);
    const onClose = vi.fn();
    rendered = renderWithFakeTransport(
      () => <NewTeamModal projectPath={projectPath} onClose={onClose} />,
      fake,
    );
    await advanceToStepThree();
    const thresholdInput = addThreshold("80");
    const repositoryInput = field('input[placeholder="https://github.com/org/repo.git"]');
    input(repositoryInput, repoUrl);
    click(button("Add Repo"));
    const createButton = button("Create");

    click(createButton);
    await waitFor(() => expect(
      document.body.querySelector(".entity-wizard-modal")?.getAttribute("aria-busy"),
    ).toBe("true"));
    const captured = JSON.stringify(fake.lastCall("create_team")?.args);
    expect(fake.callsFor("create_team")).toHaveLength(1);
    expect(thresholdInput.disabled).toBe(true);
    expect(repositoryInput.disabled).toBe(true);
    expect(button("Add threshold").disabled).toBe(true);
    expect(button("Add Repo").disabled).toBe(true);
    expect(button("Back").disabled).toBe(true);
    expect(createButton.disabled).toBe(true);
    expect(Array.from(document.body.querySelectorAll<HTMLInputElement>(
      '.wizard-repo-card input[type="checkbox"]',
    )).every((element) => element.disabled)).toBe(true);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    click(createButton);
    input(thresholdInput, "90");
    expect(fake.callsFor("create_team")).toHaveLength(1);
    expect(JSON.stringify(fake.lastCall("create_team")?.args)).toBe(captured);
    expect(onClose).not.toHaveBeenCalled();

    pending.reject(new Error("deferred rejection"));
    await waitFor(() => expect(
      document.body.querySelector('[role="alert"][aria-label="Team creation error"]')?.textContent,
    ).toContain("deferred rejection"));
    expect(document.body.querySelector(".entity-wizard-modal")?.getAttribute("aria-busy"))
      .toBe("false");
    expect(thresholdInput.disabled).toBe(false);
    expect(thresholdInput.value).toBe("80");
    expect(fake.callsFor("discover_project")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
  });
});
