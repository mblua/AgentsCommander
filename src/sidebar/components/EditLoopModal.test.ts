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

function loopWith(overrides: Partial<AcLoopSummary>): AcLoopSummary {
  return { ...loopSummary, ...overrides };
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

let dispose: (() => void) | undefined;

function renderModal(loop: AcLoopSummary = loopSummary, onClose: () => void = () => {}): void {
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(
    () =>
      EditLoopModal({
        projectPath: "C:\\Project",
        workgroups: workgroups(),
        loop,
        onClose,
      }),
    root,
  );
}

function changeInput(selector: string, value: string): void {
  const input = document.querySelector<HTMLInputElement>(selector);
  if (!input) throw new Error(`Missing input ${selector}`);
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

async function clickSave(): Promise<void> {
  await makePreviewReady();
  document.querySelector<HTMLButtonElement>('[data-ac-testid="loop.edit.save"]')?.click();
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
    dispose?.();
    dispose = undefined;
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  it("loads the full prompt body and sends an empty payload for a no-op save", async () => {
    renderModal();
    await settle();

    expect(LoopAPI.getConfig).toHaveBeenCalledWith("C:\\Project", "weekday-standup");
    expect(document.querySelector<HTMLTextAreaElement>('[data-ac-testid="loop.edit.prompt"]')?.value).toBe(
      "Full prompt body from config",
    );

    await clickSave();

    expect(LoopAPI.update).toHaveBeenCalledWith("C:\\Project", "weekday-standup", {});
  });

  it("sends only the changed name for a name-only edit", async () => {
    renderModal();
    await settle();

    changeInput('[data-ac-testid="loop.edit.name"]', "Renamed standup");
    await clickSave();

    expect(LoopAPI.update).toHaveBeenCalledWith("C:\\Project", "weekday-standup", {
      name: "Renamed standup",
    });
  });

  it("exposes selector-addressable cancel and closes without saving changes", async () => {
    const onClose = vi.fn();
    renderModal(loopSummary, onClose);
    await settle();

    changeInput('[data-ac-testid="loop.edit.name"]', "Renamed standup");
    document.querySelector<HTMLButtonElement>('[data-ac-testid="loop.edit.cancel"]')?.click();

    expect(onClose).toHaveBeenCalledOnce();
    expect(LoopAPI.update).not.toHaveBeenCalled();
  });

  it("preserves the loaded current forceInject policy when the summary prop is stale", async () => {
    m.getConfig.mockResolvedValue({
      summary: loopWith({ busyCoordinator: "forceInject" }),
      promptBody: "Full prompt body from config",
    });

    renderModal(loopWith({ busyCoordinator: "skip" }));
    await settle();

    expect(document.querySelector<HTMLInputElement>('[data-ac-testid="loop.edit.forceInject"]')?.checked).toBe(true);
    await clickSave();

    expect(LoopAPI.update).toHaveBeenCalledWith("C:\\Project", "weekday-standup", {});
  });

  it("preserves loaded skip policy without sending busyCoordinator when the checkbox is untouched", async () => {
    renderModal(loopWith({ busyCoordinator: "skip" }));
    await settle();

    expect(document.querySelector<HTMLInputElement>('[data-ac-testid="loop.edit.forceInject"]')?.checked).toBe(false);
    expect(document.body.textContent).toContain("Existing busy policy is skip");
    await clickSave();

    expect(LoopAPI.update).toHaveBeenCalledWith("C:\\Project", "weekday-standup", {});
  });
});
