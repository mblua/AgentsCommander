// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import EditTeamModal from "./EditTeamModal";
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
const teamName = "dev-team";
const agentPath = `${projectPath}\\.ac\\_agent_dev-webpage-ui`;
const repoUrl = "https://github.com/acme/repo.git";

interface Deferred<T> {
  promise: Promise<T>;
  reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let rejectPromise!: (reason: unknown) => void;
  const promise = new Promise<T>((_resolve, reject) => {
    rejectPromise = reject;
  });
  return { promise, reject: rejectPromise };
}

function teamConfig(contextAlertPercentages?: unknown): Record<string, unknown> {
  const config: Record<string, unknown> = {
    agents: ["_agent_dev-webpage-ui"],
    coordinator: "_agent_dev-webpage-ui",
    repos: [],
  };
  if (arguments.length > 0) config.contextAlertPercentages = contextAlertPercentages;
  return config;
}

function setupTransport(fake: FakeTransport, config: unknown): void {
  fake.resolve("list_all_agents", [
    { name: "dev-webpage-ui", path: agentPath, projectName: "Project" },
  ]);
  fake.resolve("get_team_config", config);
  fake.resolve("update_team", undefined);
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

function thresholdInputs(): HTMLInputElement[] {
  return Array.from(document.body.querySelectorAll<HTMLInputElement>(
    ".team-context-alert-input",
  ));
}

async function advanceToStepThree(): Promise<void> {
  await waitFor(() => expect(button("Next")).toBeTruthy());
  click(button("Next"));
  await waitFor(() => expect(button("Next").disabled).toBe(false));
  click(button("Next"));
  await waitFor(() => expect(button("Add threshold")).toBeTruthy());
}

describe("EditTeamModal context alerts", () => {
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

  it("hydrates legacy empty and valid unordered policies through EntityAPI without reloading", async () => {
    const legacyFake = new FakeTransport();
    setupTransport(legacyFake, teamConfig());
    rendered = renderWithFakeTransport(
      () => <EditTeamModal projectPath={projectPath} teamName={teamName} onClose={() => undefined} />,
      legacyFake,
    );
    await advanceToStepThree();
    expect(thresholdInputs()).toHaveLength(0);
    expect(document.body.textContent).toContain("No context alerts configured.");
    expect(legacyFake.callsFor("get_team_config")).toHaveLength(1);

    rendered.cleanup();
    rendered = null;
    document.body.replaceChildren();
    resetUiStoresForTests();

    const populatedFake = new FakeTransport();
    setupTransport(populatedFake, teamConfig([90, 50, 75]));
    rendered = renderWithFakeTransport(
      () => <EditTeamModal projectPath={projectPath} teamName={teamName} onClose={() => undefined} />,
      populatedFake,
    );
    await advanceToStepThree();
    expect(thresholdInputs().map((element) => element.value)).toEqual(["50", "75", "90"]);
    expect(populatedFake.callsFor("get_team_config")).toHaveLength(1);
    expect(populatedFake.callsFor("update_team")).toHaveLength(0);
  });

  it("preserves an unchanged populated policy during a repository edit and sends complete state", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, teamConfig([90, 50, 75]));
    const onClose = vi.fn();
    rendered = renderWithFakeTransport(
      () => <EditTeamModal projectPath={projectPath} teamName={teamName} onClose={onClose} />,
      fake,
    );
    await advanceToStepThree();
    const repositoryInput = field('input[placeholder="https://github.com/org/repo.git"]');
    input(repositoryInput, repoUrl);
    click(button("Add Repo"));
    click(button("Save"));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));

    expect(fake.callsFor("update_team")).toEqual([
      {
        cmd: "update_team",
        args: {
          projectPath,
          teamName,
          agents: ["_agent_dev-webpage-ui"],
          coordinator: "_agent_dev-webpage-ui",
          repos: [{ url: repoUrl, agents: ["_agent_dev-webpage-ui"] }],
          contextAlertPercentages: [50, 75, 90],
        },
      },
    ]);
    expect(fake.callsFor("discover_project")).toHaveLength(1);
  });

  it("clears the final row with an explicit empty update array", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, teamConfig([50]));
    const onClose = vi.fn();
    rendered = renderWithFakeTransport(
      () => <EditTeamModal projectPath={projectPath} teamName={teamName} onClose={onClose} />,
      fake,
    );
    await advanceToStepThree();
    click(button("Remove"));
    expect(thresholdInputs()).toHaveLength(0);
    click(button("Save"));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(fake.lastCall("update_team")?.args.contextAlertPercentages).toEqual([]);
  });

  it("preserves every product-invalid numeric row until the user corrects it", async () => {
    const cases: {
      values: number[];
      expectedRaw: string[];
      correct: () => void;
    }[] = [
      {
        values: [80, 80],
        expectedRaw: ["80", "80"],
        correct: () => input(thresholdInputs()[1]!, "90"),
      },
      {
        values: [50.5],
        expectedRaw: ["50.5"],
        correct: () => input(thresholdInputs()[0]!, "50"),
      },
      {
        values: [101],
        expectedRaw: ["101"],
        correct: () => input(thresholdInputs()[0]!, "100"),
      },
      {
        values: [10, 20, 30, 40],
        expectedRaw: ["10", "20", "30", "40"],
        correct: () => {
          const removeFourth = document.body.querySelector('[aria-label="Remove threshold 4"]');
          if (!(removeFourth instanceof HTMLButtonElement)) throw new Error("Fourth remove missing");
          click(removeFourth);
        },
      },
    ];

    for (const testCase of cases) {
      const fake = new FakeTransport();
      setupTransport(fake, teamConfig(testCase.values));
      rendered = renderWithFakeTransport(
        () => <EditTeamModal projectPath={projectPath} teamName={teamName} onClose={() => undefined} />,
        fake,
      );
      await advanceToStepThree();
      expect(thresholdInputs().map((element) => element.value)).toEqual(testCase.expectedRaw);
      expect(button("Save").disabled).toBe(true);
      click(button("Save"));
      expect(fake.callsFor("update_team")).toHaveLength(0);

      testCase.correct();
      expect(button("Save").disabled).toBe(false);

      rendered.cleanup();
      rendered = null;
      document.body.replaceChildren();
      resetUiStoresForTests();
    }
  });

  it("shows only the load-error state and Cancel for rejected or unrepresentable config", async () => {
    const cases: { configure: (fake: FakeTransport) => void; message: string }[] = [
      {
        configure: (fake) => {
          setupTransport(fake, teamConfig([]));
          fake.reject("get_team_config", "load rejected");
        },
        message: "load rejected",
      },
      {
        configure: (fake) => setupTransport(fake, teamConfig(null)),
        message:
          "Invalid get_team_config response: contextAlertPercentages must be an array of finite numbers",
      },
    ];

    for (const testCase of cases) {
      const fake = new FakeTransport();
      testCase.configure(fake);
      rendered = renderWithFakeTransport(
        () => <EditTeamModal projectPath={projectPath} teamName={teamName} onClose={() => undefined} />,
        fake,
      );
      await waitFor(() => expect(
        document.body.querySelector('[role="alert"][aria-label="Team configuration load error"]')
          ?.textContent,
      ).toContain(testCase.message));
      expect(Array.from(document.body.querySelectorAll(".new-agent-footer button")).map(
        (candidate) => candidate.textContent?.trim(),
      )).toEqual(["Cancel"]);
      expect(document.body.textContent).not.toContain("Add threshold");
      expect(document.body.textContent).not.toContain("Save");
      expect(fake.callsFor("update_team")).toHaveLength(0);

      rendered.cleanup();
      rendered = null;
      document.body.replaceChildren();
      resetUiStoresForTests();
    }
  });

  it("keeps save errors and all drafts separate from later editor changes", async () => {
    const fake = new FakeTransport();
    const config = teamConfig([80]);
    config.repos = [{ url: repoUrl, agents: ["_agent_dev-webpage-ui"] }];
    setupTransport(fake, config);
    fake.reject("update_team", "save rejected");
    const onClose = vi.fn();
    rendered = renderWithFakeTransport(
      () => <EditTeamModal projectPath={projectPath} teamName={teamName} onClose={onClose} />,
      fake,
    );
    await advanceToStepThree();
    const thresholdInput = thresholdInputs()[0];
    if (!thresholdInput) throw new Error("Threshold input missing");
    click(button("Save"));
    await waitFor(() => expect(
      document.body.querySelector('[role="alert"][aria-label="Team save error"]')?.textContent,
    ).toContain("save rejected"));
    expect(document.body.textContent).toContain("repo");
    expect(fake.callsFor("discover_project")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();

    input(thresholdInput, "81");
    expect(document.body.querySelector('[role="alert"][aria-label="Team save error"]')?.textContent)
      .toContain("save rejected");
    expect(thresholdInput.value).toBe("81");
  });

  it("locks all step-three interactions around one immutable deferred save", async () => {
    const fake = new FakeTransport();
    const config = teamConfig([80]);
    config.repos = [{ url: repoUrl, agents: ["_agent_dev-webpage-ui"] }];
    setupTransport(fake, config);
    const pending = deferred<void>();
    fake.onInvoke("update_team", () => pending.promise);
    const onClose = vi.fn();
    rendered = renderWithFakeTransport(
      () => <EditTeamModal projectPath={projectPath} teamName={teamName} onClose={onClose} />,
      fake,
    );
    await advanceToStepThree();
    const thresholdInput = thresholdInputs()[0];
    if (!thresholdInput) throw new Error("Threshold input missing");
    const repositoryInput = field('input[placeholder="https://github.com/org/repo.git"]');
    const saveButton = button("Save");

    click(saveButton);
    await waitFor(() => expect(
      document.body.querySelector(".entity-wizard-modal")?.getAttribute("aria-busy"),
    ).toBe("true"));
    const captured = JSON.stringify(fake.lastCall("update_team")?.args);
    expect(fake.callsFor("update_team")).toHaveLength(1);
    expect(thresholdInput.disabled).toBe(true);
    expect(repositoryInput.disabled).toBe(true);
    expect(button("Add threshold").disabled).toBe(true);
    expect(button("Add Repo").disabled).toBe(true);
    expect(button("Back").disabled).toBe(true);
    expect(saveButton.disabled).toBe(true);
    expect(Array.from(document.body.querySelectorAll<HTMLInputElement>(
      '.wizard-repo-card input[type="checkbox"]',
    )).every((element) => element.disabled)).toBe(true);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    click(saveButton);
    input(thresholdInput, "90");
    expect(fake.callsFor("update_team")).toHaveLength(1);
    expect(JSON.stringify(fake.lastCall("update_team")?.args)).toBe(captured);
    expect(onClose).not.toHaveBeenCalled();

    pending.reject(new Error("deferred save rejection"));
    await waitFor(() => expect(
      document.body.querySelector('[role="alert"][aria-label="Team save error"]')?.textContent,
    ).toContain("deferred save rejection"));
    expect(document.body.querySelector(".entity-wizard-modal")?.getAttribute("aria-busy"))
      .toBe("false");
    expect(thresholdInput.disabled).toBe(false);
    expect(thresholdInput.value).toBe("80");
    expect(fake.callsFor("discover_project")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
  });
});
