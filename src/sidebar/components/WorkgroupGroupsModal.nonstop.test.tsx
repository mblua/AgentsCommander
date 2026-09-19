// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import type { WorkgroupGroupsConfig } from "../../shared/types";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { baseSettings, click, resetUiStoresForTests, waitFor } from "../../shared/testing/ui-harness";
import { defaultGroupsConfig, defaultNonStop, workgroupGroupsStore } from "../stores/workgroup-groups";
import WorkgroupGroupsModal from "./WorkgroupGroupsModal";

// #777 modal Non-stop section: the modal-local cloneConfig BLOCKER (dev-webpage-ui
// #1) and the no-bots inline warning (Grinch G4).

// #2182 Phase 4: the Non-stop Sound Alert is platform-independent now, so the modal
// must not gate its sound controls on isWindows. The whole file runs with
// `isWindows: false`, i.e. as a non-Windows host; the #777 cases above never read
// isWindows, so pinning it false here cannot mask a regression in them.
vi.mock("../../shared/platform", async (orig) => ({
  ...(await orig<typeof import("../../shared/platform")>()),
  isWindows: false,
}));

const projectPath = "C:\\Project";

function target<T extends Element>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function mountModal() {
  const root = document.createElement("div");
  document.body.appendChild(root);
  const dispose = render(
    () => <WorkgroupGroupsModal projectPath={projectPath} projectName="Project" onClose={() => {}} />,
    root
  );
  return () => {
    dispose();
    root.remove();
  };
}

describe("#777 WorkgroupGroupsModal Non-stop section", () => {
  let restoreTransport: (() => void) | null = null;

  beforeEach(() => {
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
  });

  afterEach(() => {
    restoreTransport?.();
    restoreTransport = null;
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
    document.body.replaceChildren();
  });

  it("preserves a persisted nonStop on Save even when the user does not touch it (modal-local cloneConfig, BLOCKER)", async () => {
    const persisted = {
      ...defaultNonStop(),
      show: true,
      name: "Watcher",
      regex: "^wg-1-",
      toleranceSeconds: 45,
      telegram: { enabled: true, botId: "bot-1" },
      sound: { enabled: false, seconds: 5 },
    };
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", { ...defaultGroupsConfig(), nonStop: persisted });
    fake.resolve("get_settings", baseSettings({ telegramBots: [] }));
    fake.onInvoke("update_project_groups", (args) => args.config);
    restoreTransport = __setTransportForTests(fake);

    await workgroupGroupsStore.ensureLoaded(projectPath);
    const unmount = mountModal();
    try {
      // Save immediately, without touching the Non-stop section.
      click(target<HTMLButtonElement>("workgroupGroups.save")!);
      await waitFor(() => {
        const sent = fake.lastCall("update_project_groups")?.args.config as WorkgroupGroupsConfig | undefined;
        expect(sent?.nonStop).toMatchObject({
          show: true,
          name: "Watcher",
          regex: "^wg-1-",
          toleranceSeconds: 45,
          telegram: { enabled: true, botId: "bot-1" },
        });
      });
    } finally {
      unmount();
    }
  });

  it("shows the inline no-bots warning when Telegram is enabled but no bots are configured (Grinch G4)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", {
      ...defaultGroupsConfig(),
      nonStop: { ...defaultNonStop(), show: true, telegram: { enabled: true, botId: null } },
    });
    fake.resolve("get_settings", baseSettings({ telegramBots: [] }));
    fake.onInvoke("update_project_groups", (args) => args.config);
    restoreTransport = __setTransportForTests(fake);

    await workgroupGroupsStore.ensureLoaded(projectPath);
    const unmount = mountModal();
    try {
      await waitFor(() => expect(target("workgroupGroups.nonstop.telegramNoBots")).not.toBeNull());
    } finally {
      unmount();
    }
  });
});

describe("#2182 WorkgroupGroupsModal Non-stop sound is platform-independent", () => {
  let restoreTransport: (() => void) | null = null;

  beforeEach(() => {
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
  });

  afterEach(() => {
    restoreTransport?.();
    restoreTransport = null;
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
    document.body.replaceChildren();
  });

  async function mountWithSoundEnabled(enabled: boolean) {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", {
      ...defaultGroupsConfig(),
      nonStop: { ...defaultNonStop(), show: true, sound: { enabled, seconds: 5 } },
    });
    fake.resolve("get_settings", baseSettings({ telegramBots: [] }));
    fake.onInvoke("update_project_groups", (args) => args.config);
    restoreTransport = __setTransportForTests(fake);
    await workgroupGroupsStore.ensureLoaded(projectPath);
    return mountModal();
  }

  it("leaves the sound checkbox enabled off Windows", async () => {
    const unmount = await mountWithSoundEnabled(false);
    try {
      const checkbox = target<HTMLInputElement>("workgroupGroups.nonstop.soundEnabled");
      expect(checkbox).not.toBeNull();
      expect(checkbox!.disabled).toBe(false);
    } finally {
      unmount();
    }
  });

  it("does not render the retired platform hint", async () => {
    const unmount = await mountWithSoundEnabled(false);
    try {
      expect(target("workgroupGroups.nonstop.soundWindowsOnly")).toBeNull();
    } finally {
      unmount();
    }
  });

  it("keeps the seconds input tied to the measure, not to the platform", async () => {
    const unmount = await mountWithSoundEnabled(false);
    try {
      expect(target<HTMLInputElement>("workgroupGroups.nonstop.soundSeconds")!.disabled).toBe(true);
      click(target<HTMLInputElement>("workgroupGroups.nonstop.soundEnabled")!);
      await waitFor(() =>
        expect(target<HTMLInputElement>("workgroupGroups.nonstop.soundSeconds")!.disabled).toBe(false)
      );
    } finally {
      unmount();
    }
  });
});
