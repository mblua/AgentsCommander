// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AppSettings, WatcherConfig, WatcherEntry } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import SettingsModal, { resolveSettingsSection } from "./SettingsModal";
import { mergeSettingsForSavePreservingProjects } from "./settings-save";
import { newWatcherConfig } from "./settings-watchers";

/**
 * #1171 test 59.
 *
 * `resolveSettingsSection` falls back to `"general"` for anything it does not know, with no
 * error and no log. That silence is why this is pinned: the watcher activity window's
 * day-one empty state offers a "Configure watchers" button, and an unwired section would
 * land the user on General with nothing to explain it.
 *
 * The same gap is why `"resources"` is worth noticing: it is a member of `SettingsTab` and
 * an entry in `TABS`, the Resource Monitor asks for it by name, and it resolves to
 * `"general"` all the same. That is the broken precedent this test exists to not repeat.
 */
describe("resolveSettingsSection (#1171)", () => {
  it('maps "watchers" to the Watchers tab and not to General', () => {
    expect(resolveSettingsSection("watchers")).toBe("watchers");
  });

  it("keeps every section that already resolved", () => {
    expect(resolveSettingsSection("agents")).toBe("agents");
    expect(resolveSettingsSection("profiles")).toBe("agents");
    expect(resolveSettingsSection("integrations")).toBe("integrations");
  });

  it("still falls back to General for an unknown or absent section", () => {
    expect(resolveSettingsSection(undefined)).toBe("general");
    expect(resolveSettingsSection("nope")).toBe("general");
  });
});

/**
 * #1171 - the editor must not delete what it could not read.
 *
 * The Rust `WatcherEntry` is untagged so a hand-written `"mode": "State"` costs one skipped
 * watcher instead of the whole `AppSettings` parse, and the invalid value is kept verbatim
 * so a save round-trips the user's bytes. That guarantee only survives if the frontend's
 * save path carries the entry through untouched, which is what this pins.
 */
describe("saving a settings draft that holds an unreadable watcher (#1171)", () => {
  const unreadable = { mode: "State", commands: "claude" } as WatcherEntry;

  it("carries the unreadable entry and the valid one through the save merge", () => {
    const draft = baseSettings({
      watchers: {
        broken: unreadable,
        good: newWatcherConfig(),
      },
    });

    const merged = mergeSettingsForSavePreservingProjects(draft, baseSettings(), draft);

    expect(Object.keys(merged.watchers ?? {}).sort()).toEqual(["broken", "good"]);
    expect(merged.watchers?.broken).toEqual(unreadable);
    expect(merged.watchers?.good).toEqual(newWatcherConfig());
  });

  it("leaves the map absent when nothing was ever configured", () => {
    const merged = mergeSettingsForSavePreservingProjects(
      baseSettings(),
      baseSettings(),
      baseSettings()
    );
    expect(merged.watchers).toBeUndefined();
  });
});

/**
 * #1171 - "All agents" and an empty "Selected" are opposites, and they have to stay
 * opposites all the way to the saved draft.
 *
 * `null` reaches every configured agent; `[]` reaches none. A plain multiselect cannot
 * express both, which is why the selector is a mode. The other tests pin the pure
 * functions; this one pins the payload that actually leaves the modal.
 */
describe("the watcher commands selector, through a real save (#1171)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  function watcherSettings(commands: WatcherConfig["commands"]): AppSettings {
    return baseSettings({
      watchers: { probe: { ...newWatcherConfig(), pattern: "Read", commands } },
    });
  }

  async function savedWatcher(
    initial: AppSettings,
    startsAs: "all" | "selected",
    pick: (root: HTMLElement) => void
  ): Promise<WatcherEntry | undefined> {
    const fake = new FakeTransport();
    fake.resolve("get_settings", initial);
    fake.resolve("get_web_server_status", false);
    fake.resolve("preview_watcher_reach", []);
    fake.resolve("preview_watcher_pattern", {
      compiles: true,
      error: null,
      sampled: false,
      matchedRows: 0,
      totalRows: 0,
      samples: [],
      capturesVolatile: false,
    });
    fake.resolve("save_settings_draft", undefined);

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      // Wait for THIS test's settings, not the seed the modal paints from
      // `settingsStore.current` while `get_settings` is still in flight.
      await waitFor(() =>
        expect(
          rendered.root.querySelector<HTMLSelectElement>(
            '[data-ac-testid="settings.watchers.selectorMode.probe"]'
          )?.value
        ).toBe(startsAs)
      );

      pick(rendered.root);

      rendered.root
        .querySelector<HTMLButtonElement>('[data-ac-testid="settings.save"]')!
        .click();

      await waitFor(() => expect(fake.lastCall("save_settings_draft")).toBeTruthy());
      const draft = fake.lastCall("save_settings_draft")!.args as { draft: AppSettings };
      return draft.draft.watchers?.probe;
    } finally {
      rendered.cleanup();
    }
  }

  function selectMode(root: HTMLElement, value: "all" | "selected") {
    const select = root.querySelector<HTMLSelectElement>(
      '[data-ac-testid="settings.watchers.selectorMode.probe"]'
    )!;
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  }

  it('saves null when the user picks "All agents"', async () => {
    const saved = await savedWatcher(watcherSettings(["claude"]), "selected", (root) =>
      selectMode(root, "all")
    );
    expect((saved as WatcherConfig).commands).toBeNull();
  });

  it('saves an empty list, not null, when "Selected" is left empty', async () => {
    const saved = await savedWatcher(watcherSettings(null), "all", (root) =>
      selectMode(root, "selected")
    );
    expect((saved as WatcherConfig).commands).toEqual([]);
  });

  // A malformed entry must cost one watcher, not the tab and not the file.
  it("renders the readable watchers, names the unreadable one, and edits neither by accident", async () => {
    const fake = new FakeTransport();
    fake.resolve(
      "get_settings",
      baseSettings({
        watchers: {
          broken: { mode: "State", commands: "claude" } as WatcherEntry,
          good: { ...newWatcherConfig(), pattern: "Read" },
        },
      })
    );
    fake.resolve("get_web_server_status", false);
    fake.resolve("preview_watcher_reach", []);

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.row.good"]')
        ).toBeTruthy()
      );

      expect(
        rendered.root.querySelector('[data-ac-testid="settings.watchers.row.broken"]')
      ).toBeNull();

      const notice = rendered.root.querySelector(
        '[data-ac-testid="settings.watchers.unreadable"]'
      );
      expect(notice?.textContent).toContain("broken");
    } finally {
      rendered.cleanup();
    }
  });
});
