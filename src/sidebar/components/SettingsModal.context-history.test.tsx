// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

// #3532/#4049 - both platforms are explicit: the seam flips `isTauri` per test.
const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../../shared/platform", () => ({
  get isTauri() { return platform.tauri; },
  get isBrowser() { return !platform.tauri; },
  isWindows: false,
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setAlwaysOnTop: async () => {} }),
}));

import SettingsModal, { resolveSettingsSection } from "./SettingsModal";
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
const ENABLED = tid("settings.contextHistory.enabled");
const RETENTION = tid("settings.contextHistory.retentionDays");
const RETENTION_ERROR = tid("settings.contextHistory.retentionDays.error");
const COLOR = tid("settings.contextHistory.colorDays");
const TAB = tid("settings.tab.context-history");
const SAVE = tid("settings.save");
const CTX_KEYS = [
  "contextHistoryEnabled",
  "contextHistoryRetentionDays",
  "contextHistoryColorDays",
] as const;

describe("SettingsModal Context Heat Map section (#4049)", () => {
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

  async function renderModal(
    overrides: Partial<AppSettings> = {},
    { omit = false, section = "context-history" }: { omit?: boolean; section?: string } = {},
  ) {
    const fake = new FakeTransport();
    const snapshot = settingsSnapshot(overrides);
    if (omit) for (const key of CTX_KEYS) delete snapshot[key];
    snapshot.settingsFilePath = "C:/instance/settings.json";
    fake.resolve("get_settings", snapshot);
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    fake.resolve("save_settings_draft", undefined);
    const onClose = vi.fn();
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section={section} onClose={onClose} />,
      fake,
    );
    const $ = <T extends Element = HTMLElement>(sel: string) => rendered.root.querySelector<T>(sel);
    await waitFor(() => expect($(tid("settings.tab.general"))).toBeTruthy());
    await waitFor(() => expect(rendered.root.querySelector(".modal-body")).toBeTruthy());
    return { fake, rendered, onClose, $ };
  }

  const fire = (el: HTMLInputElement | HTMLSelectElement, value: string, event: string) => {
    el.value = value;
    el.dispatchEvent(new Event(event, { bubbles: true }));
  };
  const toggle = (box: HTMLInputElement) => {
    box.checked = !box.checked;
    box.dispatchEvent(new Event("change", { bubbles: true }));
  };
  const savedDraft = (fake: FakeTransport) =>
    (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings }).draft;
  const save = async (m: Awaited<ReturnType<typeof renderModal>>) => {
    m.$<HTMLButtonElement>(SAVE)!.click();
    await waitFor(() => expect(m.fake.lastCall("save_settings_draft")).toBeTruthy());
    return savedDraft(m.fake);
  };

  it("routes the section only on desktop; unknown sections stay on General", () => {
    expect(resolveSettingsSection("context-history")).toBe("context-history");
    expect(resolveSettingsSection("nope")).toBe("general");
    platform.tauri = false;
    expect(resolveSettingsSection("context-history")).toBe("general");
  });

  it("resolves missing preferences to off, 30 days and a 30-day color window", async () => {
    const m = await renderModal({}, { omit: true });
    try {
      await waitFor(() => expect(m.$(ENABLED)).toBeTruthy());
      expect(m.$(TAB)!.getAttribute("data-ac-state")).toBe("active");
      expect(m.$<HTMLInputElement>(ENABLED)!.checked).toBe(false);
      expect(m.$<HTMLInputElement>(RETENTION)!.value).toBe("30");
      expect(m.$<HTMLSelectElement>(COLOR)!.value).toBe("30");
      expect(m.$(RETENTION_ERROR)).toBeNull();
    } finally { m.rendered.cleanup(); }
  });

  for (const days of [1, 7, 30] as const) {
    it(`restores saved values with a ${days}-day color window`, async () => {
      const m = await renderModal({
        contextHistoryEnabled: true,
        contextHistoryRetentionDays: 90,
        contextHistoryColorDays: days,
      });
      try {
        await waitFor(() => expect(m.$<HTMLInputElement>(RETENTION)?.value).toBe("90"));
        expect(m.$<HTMLInputElement>(ENABLED)!.checked).toBe(true);
        expect(m.$<HTMLSelectElement>(COLOR)!.value).toBe(String(days));
      } finally { m.rendered.cleanup(); }
    });
  }

  it("saves enable, numeric retention and color, keeping unrelated fields", async () => {
    const m = await renderModal(
      { contextHistoryColorDays: 30, roomIdleConfirmationSeconds: 60 },
      { omit: false },
    );
    try {
      await waitFor(() => expect(m.$(ENABLED)).toBeTruthy());
      toggle(m.$<HTMLInputElement>(ENABLED)!);
      fire(m.$<HTMLInputElement>(RETENTION)!, "45", "input");
      fire(m.$<HTMLSelectElement>(COLOR)!, "7", "change");
      const draft = await save(m);
      expect(draft.contextHistoryEnabled).toBe(true);
      expect(draft.contextHistoryRetentionDays).toBe(45);
      expect(typeof draft.contextHistoryRetentionDays).toBe("number");
      expect(draft.contextHistoryColorDays).toBe(7);
      expect(draft.roomIdleConfirmationSeconds).toBe(60);
    } finally { m.rendered.cleanup(); }
  });

  it("disabling saves false without touching retention or color", async () => {
    const m = await renderModal({
      contextHistoryEnabled: true,
      contextHistoryRetentionDays: 12,
      contextHistoryColorDays: 1,
    });
    try {
      await waitFor(() => expect(m.$<HTMLInputElement>(ENABLED)?.checked).toBe(true));
      toggle(m.$<HTMLInputElement>(ENABLED)!);
      const draft = await save(m);
      expect(draft.contextHistoryEnabled).toBe(false);
      expect(draft.contextHistoryRetentionDays).toBe(12);
      expect(draft.contextHistoryColorDays).toBe(1);
    } finally { m.rendered.cleanup(); }
  });

  for (const days of [1, 365]) {
    it(`saves the ${days}-day bound as a number`, async () => {
      const m = await renderModal();
      try {
        await waitFor(() => expect(m.$(RETENTION)).toBeTruthy());
        fire(m.$<HTMLInputElement>(RETENTION)!, String(days), "input");
        expect(m.$(RETENTION_ERROR)).toBeNull();
        expect((await save(m)).contextHistoryRetentionDays).toBe(days);
      } finally { m.rendered.cleanup(); }
    });
  }

  for (const raw of ["", "0", "366", "2.5", "-1"]) {
    it(`blocks Save for retention ${JSON.stringify(raw)} and keeps the text`, async () => {
      const m = await renderModal({ contextHistoryRetentionDays: 30 });
      try {
        await waitFor(() => expect(m.$(RETENTION)).toBeTruthy());
        const input = m.$<HTMLInputElement>(RETENTION)!;
        fire(input, raw, "input");
        await waitFor(() =>
          expect(m.$(RETENTION_ERROR)?.textContent).toContain("Context Heat Map"),
        );
        const saveBtn = m.$<HTMLButtonElement>(SAVE)!;
        expect(saveBtn.disabled).toBe(true);
        saveBtn.click();
        expect(m.fake.lastCall("save_settings_draft")).toBeUndefined();
        expect(input.value).toBe(raw);
        expect(m.onClose).not.toHaveBeenCalled();
      } finally { m.rendered.cleanup(); }
    });
  }

  it("a draft edit does not reach the published settings; Cancel saves nothing", async () => {
    const m = await renderModal({ contextHistoryEnabled: false, contextHistoryColorDays: 30 });
    try {
      await waitFor(() => expect(m.$(ENABLED)).toBeTruthy());
      const before = { ...settingsStore.current };
      toggle(m.$<HTMLInputElement>(ENABLED)!);
      fire(m.$<HTMLInputElement>(RETENTION)!, "5", "input");
      fire(m.$<HTMLSelectElement>(COLOR)!, "1", "change");
      for (const key of CTX_KEYS) expect(settingsStore.current?.[key]).toBe(before[key]);
      m.$<HTMLButtonElement>(tid("settings.cancel"))!.click();
      expect(m.onClose).toHaveBeenCalledTimes(1);
      expect(m.fake.lastCall("save_settings_draft")).toBeUndefined();
    } finally { m.rendered.cleanup(); }
  });

  it("keeps the draft and the error after a failed Save", async () => {
    const m = await renderModal({ contextHistoryColorDays: 30 });
    try {
      await waitFor(() => expect(m.$(COLOR)).toBeTruthy());
      m.fake.reject("save_settings_draft", "color window write failed");
      const select = m.$<HTMLSelectElement>(COLOR)!;
      fire(select, "7", "change");
      m.$<HTMLButtonElement>(SAVE)!.click();
      await waitFor(() =>
        expect(m.rendered.root.querySelector(".modal-save-error")?.textContent)
          .toContain("color window write failed"),
      );
      expect(select.value).toBe("7");
      expect(m.onClose).not.toHaveBeenCalled();
    } finally { m.rendered.cleanup(); }
  });

  it("the browser hides the tab and opens General for a direct section request", async () => {
    platform.tauri = false;
    const m = await renderModal({ contextHistoryEnabled: true, contextHistoryColorDays: 7 });
    try {
      await waitFor(() => expect(m.$(tid("settings.general.layout"))).toBeTruthy());
      expect(m.$(TAB)).toBeNull();
      expect(m.$(tid("settings.tab.general"))!.getAttribute("data-ac-state")).toBe("active");
      for (const sel of [ENABLED, RETENTION, COLOR]) expect(m.$(sel)).toBeNull();
    } finally { m.rendered.cleanup(); }
  });
});
