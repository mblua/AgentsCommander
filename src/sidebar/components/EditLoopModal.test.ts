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
  sessionStart: "fresh",
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
    {
      name: "wg-11-ops-team",
      path: "C:\\Project\\.ac\\wg-11-ops-team",
      task: null,
      agents: [
        {
          name: "ops-lead",
          path: "C:\\Project\\.ac\\wg-11-ops-team\\__agent_ops-lead",
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

function setCheckbox(selector: string, checked: boolean): void {
  const checkbox = document.querySelector<HTMLInputElement>(selector);
  if (!checkbox) throw new Error(`Missing checkbox ${selector}`);
  checkbox.checked = checked;
  checkbox.dispatchEvent(new Event("change", { bubbles: true }));
}

function setSelect(selector: string, value: string): void {
  const select = document.querySelector<HTMLSelectElement>(selector);
  if (!select) throw new Error(`Missing select ${selector}`);
  select.value = value;
  select.dispatchEvent(new Event("change", { bubbles: true }));
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

  it("renders the accumulate control checked or unchecked from the loaded session start", async () => {
    m.getConfig.mockResolvedValue({
      summary: loopWith({ sessionStart: "accumulate" }),
      promptBody: "Full prompt body from config",
    });

    renderModal(loopWith({ sessionStart: "accumulate" }));
    await settle();

    expect(document.querySelector<HTMLInputElement>('[data-ac-testid="loop.edit.accumulate"]')?.checked).toBe(true);

    dispose?.();
    dispose = undefined;
    document.body.innerHTML = "";

    m.getConfig.mockResolvedValue({
      summary: loopWith({ sessionStart: "fresh" }),
      promptBody: "Full prompt body from config",
    });

    renderModal(loopWith({ sessionStart: "fresh" }));
    await settle();

    expect(document.querySelector<HTMLInputElement>('[data-ac-testid="loop.edit.accumulate"]')?.checked).toBe(false);
  });

  it("sends no sessionStart key for a name-only edit", async () => {
    m.getConfig.mockResolvedValue({
      summary: loopWith({ sessionStart: "accumulate" }),
      promptBody: "Full prompt body from config",
    });

    renderModal(loopWith({ sessionStart: "accumulate" }));
    await settle();

    changeInput('[data-ac-testid="loop.edit.name"]', "Renamed standup");
    await clickSave();

    expect(LoopAPI.update).toHaveBeenCalledWith("C:\\Project", "weekday-standup", {
      name: "Renamed standup",
    });
  });

  it("sends sessionStart fresh when the accumulate control is unchecked", async () => {
    m.getConfig.mockResolvedValue({
      summary: loopWith({ sessionStart: "accumulate" }),
      promptBody: "Full prompt body from config",
    });

    renderModal(loopWith({ sessionStart: "accumulate" }));
    await settle();

    setCheckbox('[data-ac-testid="loop.edit.accumulate"]', false);
    await clickSave();

    expect(LoopAPI.update).toHaveBeenCalledWith("C:\\Project", "weekday-standup", {
      sessionStart: "fresh",
    });
  });

  it("sends sessionStart accumulate without touching busyCoordinator when the control is checked", async () => {
    renderModal();
    await settle();

    setCheckbox('[data-ac-testid="loop.edit.accumulate"]', true);
    await clickSave();

    expect(LoopAPI.update).toHaveBeenCalledWith("C:\\Project", "weekday-standup", {
      sessionStart: "accumulate",
    });
  });

  it("sends no sessionStart key for a name-only edit when the summary prop is stale", async () => {
    m.getConfig.mockResolvedValue({
      summary: loopWith({ sessionStart: "accumulate" }),
      promptBody: "Full prompt body from config",
    });

    renderModal(loopWith({ sessionStart: "fresh" }));
    await settle();

    expect(document.querySelector<HTMLInputElement>('[data-ac-testid="loop.edit.accumulate"]')?.checked).toBe(true);

    changeInput('[data-ac-testid="loop.edit.name"]', "Renamed standup");
    await clickSave();

    expect(LoopAPI.update).toHaveBeenCalledWith("C:\\Project", "weekday-standup", {
      name: "Renamed standup",
    });
  });
  function scheduleResetNotice(): HTMLElement | null {
    return document.querySelector<HTMLElement>('[data-ac-testid="loop.edit.scheduleReset"]');
  }

  it("shows the exact schedule-reset notice when the cron expression changed", async () => {
    renderModal();
    await settle();

    changeInput('[data-ac-testid="loop.edit.cron"]', "0 10 * * 1-5");
    await settle();

    expect(scheduleResetNotice()?.textContent?.trim()).toBe(
      "Saving this change restarts the schedule. The next run is counted from the moment you save.",
    );
  });

  it("shows the schedule-reset notice when the room orchestrator changed", async () => {
    renderModal();
    await settle();

    setSelect('[data-ac-testid="loop.edit.workgroup"]', "wg-11-ops-team");
    await settle();

    expect(scheduleResetNotice()).not.toBeNull();
  });

  it("shows the schedule-reset notice when the prompt body changed", async () => {
    renderModal();
    await settle();

    changeInput('[data-ac-testid="loop.edit.prompt"]', "Full prompt body from config ");
    await settle();

    expect(scheduleResetNotice()).not.toBeNull();
  });

  it("shows the schedule-reset notice when the busy policy changed", async () => {
    renderModal();
    await settle();

    setCheckbox('[data-ac-testid="loop.edit.forceInject"]', true);
    await settle();

    expect(scheduleResetNotice()).not.toBeNull();
  });

  it("shows the schedule-reset notice when the session start changed", async () => {
    renderModal();
    await settle();

    setCheckbox('[data-ac-testid="loop.edit.accumulate"]', true);
    await settle();

    expect(scheduleResetNotice()).not.toBeNull();
  });

  it("shows the schedule-reset notice when enabled changed", async () => {
    renderModal();
    await settle();

    setCheckbox('[data-ac-testid="loop.edit.enabled"]', false);
    await settle();

    expect(scheduleResetNotice()).not.toBeNull();
  });

  it("hides the schedule-reset notice for a name-only edit", async () => {
    renderModal();
    await settle();

    changeInput('[data-ac-testid="loop.edit.name"]', "Renamed standup");
    await settle();

    expect(scheduleResetNotice()).toBeNull();
  });

  it("hides the schedule-reset notice on an untouched form after load", async () => {
    renderModal();
    await settle();

    expect(scheduleResetNotice()).toBeNull();
  });

  it("hides the schedule-reset notice while the Loop config is still loading", async () => {
    m.getConfig.mockReturnValue(new Promise(() => {}));

    renderModal();
    await settle();

    expect(document.body.textContent).toContain("Loading Loop...");
    expect(scheduleResetNotice()).toBeNull();
  });

  it("hides the schedule-reset notice when a changed cron expression is reverted", async () => {
    renderModal();
    await settle();

    changeInput('[data-ac-testid="loop.edit.cron"]', "0 10 * * 1-5");
    await settle();
    expect(scheduleResetNotice()).not.toBeNull();

    changeInput('[data-ac-testid="loop.edit.cron"]', "0 9 * * 1-5");
    await settle();

    expect(scheduleResetNotice()).toBeNull();
  });
});
