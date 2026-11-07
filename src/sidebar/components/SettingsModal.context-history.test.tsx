// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

// #3532 - both platforms are explicit: the seam flips `isTauri` per test.
const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../../shared/platform", () => ({
  get isTauri() { return platform.tauri; },
  get isBrowser() { return !platform.tauri; },
  isWindows: false,
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setAlwaysOnTop: async () => {} }),
}));

import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  settingsSnapshot,
  waitFor,
} from "../../shared/testing/ui-harness";
import { settingsStore } from "../../shared/stores/settings";
import type { AppSettings } from "../../shared/types";

const tid = (id: string) => `[data-ac-testid="${id}"]`;
const FIELD = tid("settings.general.contextHistoryColorDays");
const SEARCH = tid("settings.general.search");
const RESULT = tid("settings.general.result.contextHistoryColorDays");
const SAVE = tid("settings.save");

describe("SettingsModal context history color window (#3532)", () => {
  beforeEach(() => {
    platform.tauri = true;
    const cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    return () => {
      cleanupDom();
      resetUiStoresForTests();
      document.body.replaceChildren();
    };
  });

  async function renderModal(overrides: Partial<AppSettings> = {}, omitDays = false) {
    const fake = new FakeTransport();
    const snapshot = settingsSnapshot(overrides);
    if (omitDays) delete snapshot.contextHistoryColorDays;
    snapshot.settingsFilePath = "C:/instance/settings.json";
    fake.resolve("get_settings", snapshot);
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    fake.resolve("save_settings_draft", undefined);
    const onClose = vi.fn();
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="general" onClose={onClose} />,
      fake,
    );
    await waitFor(() => expect(rendered.root.textContent).toContain(snapshot.settingsFilePath));
    const $ = <T extends Element = HTMLElement>(sel: string) => rendered.root.querySelector<T>(sel);
    return { fake, rendered, onClose, $ };
  }

  const pick = (select: HTMLSelectElement, value: string) => {
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  };
  const savedDraft = (fake: FakeTransport) =>
    (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings }).draft;

  it("resolves a missing preference to 30 days", async () => {
    const m = await renderModal({}, true);
    try { expect(m.$<HTMLSelectElement>(FIELD)!.value).toBe("30"); }
    finally { m.rendered.cleanup(); }
  });

  for (const days of [1, 7, 30] as const) {
    it(`loads a saved ${days}-day window`, async () => {
      const m = await renderModal({ contextHistoryColorDays: days });
      try {
        const select = m.$<HTMLSelectElement>(FIELD)!;
        expect(select.value).toBe(String(days));
        const wrapper = select.closest("[data-ac-setting]")!;
        expect(wrapper.getAttribute("data-ac-setting")).toBe("contextHistoryColorDays");
      } finally { m.rendered.cleanup(); }
    });
  }

  it("persists the number on Save and keeps the other fields", async () => {
    const m = await renderModal(
      { contextHistoryColorDays: 30, roomIdleConfirmationSeconds: 60 },
      false,
    );
    try {
      pick(m.$<HTMLSelectElement>(FIELD)!, "7");
      m.$<HTMLButtonElement>(SAVE)!.click();
      await waitFor(() => expect(m.fake.lastCall("save_settings_draft")).toBeTruthy());
      const draft = savedDraft(m.fake);
      expect(draft.contextHistoryColorDays).toBe(7);
      expect(typeof draft.contextHistoryColorDays).toBe("number");
      expect(draft.roomIdleConfirmationSeconds).toBe(60);
    } finally { m.rendered.cleanup(); }
  });

  it("a draft edit does not reach the published settings; Cancel saves nothing", async () => {
    const m = await renderModal({ contextHistoryColorDays: 30 });
    try {
      const before = settingsStore.current?.contextHistoryColorDays;
      pick(m.$<HTMLSelectElement>(FIELD)!, "1");
      expect(settingsStore.current?.contextHistoryColorDays).toBe(before);
      m.$<HTMLButtonElement>(tid("settings.cancel"))!.click();
      expect(m.onClose).toHaveBeenCalledTimes(1);
      expect(m.fake.lastCall("save_settings_draft")).toBeUndefined();
    } finally { m.rendered.cleanup(); }
    const reopened = await renderModal({ contextHistoryColorDays: 30 });
    try { expect(reopened.$<HTMLSelectElement>(FIELD)!.value).toBe("30"); }
    finally { reopened.rendered.cleanup(); }
  });

  it("keeps the draft and the error after a failed Save", async () => {
    const m = await renderModal({ contextHistoryColorDays: 30 });
    try {
      m.fake.reject("save_settings_draft", "color window write failed");
      const select = m.$<HTMLSelectElement>(FIELD)!;
      const before = settingsStore.current?.contextHistoryColorDays;
      pick(select, "7");
      m.$<HTMLButtonElement>(SAVE)!.click();
      await waitFor(() =>
        expect(m.rendered.root.querySelector(".modal-save-error")?.textContent)
          .toContain("color window write failed"),
      );
      expect(select.value).toBe("7");
      expect(m.onClose).not.toHaveBeenCalled();
      expect(settingsStore.current?.contextHistoryColorDays).toBe(before);
    } finally { m.rendered.cleanup(); }
  });

  it("search opens the field on desktop", async () => {
    const m = await renderModal();
    try {
      const search = m.$<HTMLInputElement>(SEARCH)!;
      search.value = "contexto media";
      search.dispatchEvent(new Event("input", { bubbles: true }));
      m.$<HTMLButtonElement>(RESULT)!.click();
      await waitFor(() => expect(document.activeElement).toBe(m.$(FIELD)));
    } finally { m.rendered.cleanup(); }
  });

  it("the browser shows neither the field nor a search result", async () => {
    platform.tauri = false;
    const m = await renderModal({ contextHistoryColorDays: 7 });
    try {
      expect(m.$(FIELD)).toBeNull();
      expect(m.$('[data-ac-setting="contextHistoryColorDays"]')).toBeNull();
      const search = m.$<HTMLInputElement>(SEARCH)!;
      search.value = "contexto media";
      search.dispatchEvent(new Event("input", { bubbles: true }));
      expect(m.$(RESULT)).toBeNull();
      search.value = "hotkey";
      search.dispatchEvent(new Event("input", { bubbles: true }));
      expect(m.$(tid("settings.general.result.sidebarCompactHotkey"))).toBeTruthy();
    } finally { m.rendered.cleanup(); }
  });
});
