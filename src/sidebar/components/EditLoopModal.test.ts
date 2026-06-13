// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import type { AcLoopSummary, AcWorkgroup } from "../../shared/types";
import { LoopAPI } from "../../shared/ipc";
import EditLoopModal from "./EditLoopModal";

const m = vi.hoisted(() => ({
  getConfig: vi.fn(),
  update: vi.fn(),
  previewCron: vi.fn(),
  reloadProject: vi.fn(),
}));

vi.mock("../../shared/ipc", () => ({
  LoopAPI: {
    getConfig: m.getConfig,
    update: m.update,
    previewCron: m.previewCron,
  },
}));

vi.mock("../stores/project", () => ({
  projectStore: {
    reloadProject: m.reloadProject,
  },
}));

const loopSummary: AcLoopSummary = {
  id: "weekday-standup",
  name: "Weekday standup",
  enabled: true,
  expr: "0 9 * * 1-5",
  timezone: "local",
  targetKind: "workgroupCoordinator",
  workgroup: "wg-10-dev-team",
  promptPreview: "Short preview",
  busyCoordinator: "skip",
  path: "C:\\Project\\.ac\\_loop_weekday-standup",
  configPath: "C:\\Project\\.ac\\_loop_weekday-standup\\config.toml",
  lastCheckedAt: null,
  lastDueAt: null,
  lastDeliveredAt: null,
  lastResult: null,
  pendingDueAt: null,
  lastMissedClosedAt: null,
  nextDueAt: null,
};

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

async function makePreviewReady(): Promise<void> {
  await settle();
  vi.advanceTimersByTime(300);
  await settle();
}

describe("EditLoopModal", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    m.getConfig.mockResolvedValue({
      summary: loopSummary,
      promptBody: "Full prompt body from config",
    });
    m.previewCron.mockResolvedValue({
      nextDueAt: "2026-06-14T12:00:00Z",
      upcoming: ["2026-06-14T12:00:00Z"],
    });
    m.update.mockResolvedValue({ summary: loopSummary, promptBody: "Full prompt body from config" });
    m.reloadProject.mockResolvedValue(undefined);
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  it("loads the full prompt body and preserves skip when the force checkbox is untouched", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        EditLoopModal({
          projectPath: "C:\\Project",
          workgroups: workgroups(),
          loop: loopSummary,
          onClose: () => {},
        }),
      root,
    );

    await settle();
    expect(LoopAPI.getConfig).toHaveBeenCalledWith("C:\\Project", "weekday-standup");
    expect(document.querySelector<HTMLTextAreaElement>('[data-ac-testid="loop.edit.prompt"]')?.value).toBe(
      "Full prompt body from config",
    );

    await makePreviewReady();
    document.querySelector<HTMLButtonElement>('[data-ac-testid="loop.edit.save"]')?.click();
    await settle();

    expect(LoopAPI.update).toHaveBeenCalledWith(
      "C:\\Project",
      "weekday-standup",
      expect.objectContaining({
        promptBody: "Full prompt body from config",
        busyCoordinator: "skip",
      }),
    );

    dispose();
  });
});
