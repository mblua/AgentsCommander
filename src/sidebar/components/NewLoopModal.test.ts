// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import type { AcWorkgroup } from "../../shared/types";
import { LoopAPI } from "../../shared/ipc";
import NewLoopModal from "./NewLoopModal";
import {
  busyPolicyFromForceCheckbox,
  coordinatorOptionsFromWorkgroups,
  hasFiveCronFields,
} from "./loop-modal-helpers";

const m = vi.hoisted(() => ({
  create: vi.fn(),
  previewCron: vi.fn(),
  reloadProject: vi.fn(),
}));

vi.mock("../../shared/ipc", () => ({
  LoopAPI: {
    create: m.create,
    previewCron: m.previewCron,
  },
}));

vi.mock("../stores/project", () => ({
  projectStore: {
    reloadProject: m.reloadProject,
  },
}));

function workgroups(): AcWorkgroup[] {
  return [
    {
      name: "wg-10-dev-team",
      path: "C:\\Project\\.ac\\wg-10-dev-team",
      task: null,
      agents: [
        {
          name: "tech-lead",
          path: "C:\\Project\\.ac\\wg-10-dev-team\\__agent_tech-lead",
          repoPaths: [],
          isCoordinator: true,
        },
      ],
    },
  ];
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

function setInput(selector: string, value: string): void {
  const input = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(selector);
  if (!input) throw new Error(`missing input ${selector}`);
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function setSelect(selector: string, value: string): void {
  const select = document.querySelector<HTMLSelectElement>(selector);
  if (!select) throw new Error(`missing select ${selector}`);
  select.value = value;
  select.dispatchEvent(new Event("change", { bubbles: true }));
}

function setCheckbox(selector: string, checked: boolean): void {
  const checkbox = document.querySelector<HTMLInputElement>(selector);
  if (!checkbox) throw new Error(`missing checkbox ${selector}`);
  checkbox.checked = checked;
  checkbox.dispatchEvent(new Event("change", { bubbles: true }));
}

async function makePreviewReady(): Promise<void> {
  await settle();
  vi.advanceTimersByTime(300);
  await settle();
}

describe("Loop modal helpers", () => {
  it("validates five-field cron expressions and coordinator options", () => {
    expect(hasFiveCronFields("0 9 * * 1-5")).toBe(true);
    expect(hasFiveCronFields("0 0 9 * * 1-5")).toBe(false);
    expect(coordinatorOptionsFromWorkgroups(workgroups())).toEqual([
      {
        workgroup: "wg-10-dev-team",
        coordinatorName: "tech-lead",
        label: "wg-10-dev-team - tech-lead",
      },
    ]);
  });

  it("maps the force checkbox to the backend busy policy values", () => {
    expect(busyPolicyFromForceCheckbox(false)).toBe("waitUntilIdle");
    expect(busyPolicyFromForceCheckbox(true)).toBe("forceInject");
  });
});

describe("NewLoopModal", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    m.previewCron.mockResolvedValue({
      nextDueAt: "2026-06-14T12:00:00Z",
      upcoming: ["2026-06-14T12:00:00Z"],
    });
    m.create.mockResolvedValue({ summary: null, promptBody: "" });
    m.reloadProject.mockResolvedValue(undefined);
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  it("disables creation when no workgroup has a coordinator", () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        NewLoopModal({
          projectPath: "C:\\Project",
          workgroups: [
            {
              name: "wg-10-dev-team",
              path: "C:\\Project\\.ac\\wg-10-dev-team",
              task: null,
              agents: [],
            },
          ],
          onClose: () => {},
        }),
      root,
    );

    expect(document.querySelector<HTMLButtonElement>('[data-ac-testid="loop.new.create"]')?.disabled).toBe(true);
    expect(document.body.textContent).toContain("verified coordinator");

    dispose();
  });

  it("creates with waitUntilIdle when force inject is unchecked", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        NewLoopModal({
          projectPath: "C:\\Project",
          workgroups: workgroups(),
          onClose: () => {},
        }),
      root,
    );

    setInput('[data-ac-testid="loop.new.name"]', "Weekday standup");
    setInput('[data-ac-testid="loop.new.cron"]', "0 9 * * 1-5");
    setSelect('[data-ac-testid="loop.new.workgroup"]', "wg-10-dev-team");
    setInput('[data-ac-testid="loop.new.prompt"]', "Summarize blockers");
    await makePreviewReady();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="loop.new.create"]')?.click();
    await settle();

    expect(LoopAPI.create).toHaveBeenCalledWith(
      "C:\\Project",
      expect.objectContaining({
        name: "Weekday standup",
        expr: "0 9 * * 1-5",
        workgroup: "wg-10-dev-team",
        promptBody: "Summarize blockers",
        busyCoordinator: "waitUntilIdle",
      }),
    );

    dispose();
  });

  it("creates with forceInject when the force checkbox is checked", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        NewLoopModal({
          projectPath: "C:\\Project",
          workgroups: workgroups(),
          onClose: () => {},
        }),
      root,
    );

    setInput('[data-ac-testid="loop.new.name"]', "Weekday standup");
    setInput('[data-ac-testid="loop.new.cron"]', "0 9 * * 1-5");
    setSelect('[data-ac-testid="loop.new.workgroup"]', "wg-10-dev-team");
    setInput('[data-ac-testid="loop.new.prompt"]', "Summarize blockers");
    setCheckbox('[data-ac-testid="loop.new.forceInject"]', true);
    await makePreviewReady();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="loop.new.create"]')?.click();
    await settle();

    expect(LoopAPI.create).toHaveBeenCalledWith(
      "C:\\Project",
      expect.objectContaining({ busyCoordinator: "forceInject" }),
    );

    dispose();
  });
});
