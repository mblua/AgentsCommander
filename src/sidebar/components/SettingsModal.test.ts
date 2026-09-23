import { describe, expect, it } from "vitest";
import type { AppSettings } from "../../shared/types";
import { mergeSettingsForSavePreservingProjects } from "./settings-save";
import { baseSettings } from "../../shared/testing/base-settings";

function settings(overrides: Partial<AppSettings>): AppSettings {
  return baseSettings({
    themeLight: true,
    soundsEnabled: true,
    teamIdleBeepEnabled: true,
    onboardingDismissed: false,
    ...overrides,
    archivedProjectPaths: overrides.archivedProjectPaths ?? [],
  });
}

describe("mergeSettingsForSavePreservingProjects", () => {
  it("preserves fresh project registration fields from disk", () => {
    const draft = settings({
      soundsEnabled: false,
      projectPaths: ["C:\\Stale"],
      projectPath: "C:\\Stale",
    });
    const fresh = settings({
      soundsEnabled: true,
      projectPaths: ["C:\\Fresh", "D:\\Other"],
      projectPath: "C:\\Fresh",
    });

    expect(mergeSettingsForSavePreservingProjects(draft, fresh)).toMatchObject({
      soundsEnabled: false,
      projectPaths: ["C:\\Fresh", "D:\\Other"],
      projectPath: "C:\\Fresh",
    });
  });

  it("keeps terminal snapshots under the dedicated setting owner", () => {
    const draft = settings({
      soundsEnabled: false,
      terminalSnapshotsEnabled: true,
    });
    const fresh = settings({
      soundsEnabled: true,
      terminalSnapshotsEnabled: false,
    });

    expect(mergeSettingsForSavePreservingProjects(draft, fresh)).toMatchObject({
      soundsEnabled: false,
      terminalSnapshotsEnabled: false,
    });
  });
});
