// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SettingsSnapshot } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { baseSettings } from "../../shared/testing/base-settings";
import { __resetTutorialProgressForTests, tutorialProgressStore } from "./tutorial-progress";
import { __resetTutorialStoreForTests, tutorialStore } from "./tutorial";

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: { get: vi.fn(), update: vi.fn(() => Promise.resolve()) },
}));
vi.mock("../../shared/stores/settings", () => ({ settingsStore: { refresh: vi.fn() } }));
vi.mock("../../sidebar/stores/project", () => ({
  projectStore: { newProjectOutcome: { seq: 3, path: null, created: false } },
}));

const get = vi.mocked(SettingsAPI.get);
const update = vi.mocked(SettingsAPI.update);
const withFlags = (flags: object = {}) =>
  ({ ...baseSettings(), ...flags }) as unknown as SettingsSnapshot;

beforeEach(() => {
  vi.clearAllMocks();
  __resetTutorialProgressForTests();
  __resetTutorialStoreForTests();
  get.mockResolvedValue(withFlags());
});
afterEach(() => vi.restoreAllMocks());

describe("tutorialProgressStore", () => {
  it("load reads both flags, once", async () => {
    get.mockResolvedValue(withFlags({ tutorialHintDismissed: true, teamTutorialCompleted: true }));
    await tutorialProgressStore.load();
    await tutorialProgressStore.load();
    expect(get).toHaveBeenCalledTimes(1);
    expect(tutorialProgressStore).toMatchObject(
      { loaded: true, hintDismissed: true, completed: true });
  });

  it("missing keys load as false", async () => {
    await tutorialProgressStore.load();
    expect(tutorialProgressStore).toMatchObject(
      { loaded: true, hintDismissed: false, completed: false });
  });

  it("markCompleted writes fresh settings plus both flags once", async () => {
    const fresh = withFlags({ theme: "x" });
    get.mockResolvedValue(fresh);
    await tutorialProgressStore.markCompleted();
    expect(update).toHaveBeenCalledTimes(1);
    expect(update).toHaveBeenCalledWith(
      { ...fresh, tutorialHintDismissed: true, teamTutorialCompleted: true });
    expect(settingsStore.refresh).toHaveBeenCalledTimes(1);
  });

  it("dismissHint writes only tutorialHintDismissed", async () => {
    await tutorialProgressStore.dismissHint();
    const arg = update.mock.calls[0]?.[0] as unknown as Record<string, unknown>;
    expect(arg.tutorialHintDismissed).toBe(true);
    expect("teamTutorialCompleted" in arg).toBe(false);
    expect(tutorialProgressStore.hintDismissed).toBe(true);
  });

  it("get failure keeps flags in memory and does not throw", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    get.mockRejectedValue(new Error("boom"));
    await expect(tutorialProgressStore.load()).resolves.toBeUndefined();
    expect(tutorialProgressStore).toMatchObject({ loaded: true, completed: false });
    await expect(tutorialProgressStore.markCompleted()).resolves.toBeUndefined();
    expect(tutorialProgressStore).toMatchObject({ hintDismissed: true, completed: true });
    expect(update).not.toHaveBeenCalled();
    expect(warn).toHaveBeenCalled();
  });
});

describe("tutorialStore", () => {
  it("start uses the current outcome seq and is a no-op when active", () => {
    tutorialStore.start();
    expect(tutorialStore.ctx).toMatchObject({ step: "openMenu", seq: 3 });
    const first = tutorialStore.ctx;
    tutorialStore.start();
    expect(tutorialStore.ctx).toBe(first);
  });

  it("13. finish marks completed before the run ends", async () => {
    tutorialStore.start();
    const order: string[] = [];
    const real = tutorialProgressStore.markCompleted.bind(tutorialProgressStore);
    vi.spyOn(tutorialProgressStore, "markCompleted").mockImplementation(() => {
      const p = real();
      order.push(`completed=${tutorialProgressStore.completed} active=${tutorialStore.active}`);
      return p;
    });
    const done = tutorialStore.finish();
    order.push(`after: active=${tutorialStore.active}`);
    await done;
    expect(order).toEqual(["completed=true active=true", "after: active=false"]);
  });
});
