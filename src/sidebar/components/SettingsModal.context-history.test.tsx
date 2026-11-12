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
const COLOR = tid("settings.contextHistory.colorSessions");
const WINDOWS = tid("settings.contextHistory.sessionWindows");
const WINDOWS_ERROR = tid("settings.contextHistory.sessionWindows.error");
const TAB = tid("settings.tab.context-history");
const SAVE = tid("settings.save");
const CTX_KEYS = [
  "contextHistoryEnabled",
  "contextHistoryRetentionDays",
  "contextHistorySessionWindows",
  "contextHistoryColorSessions",
] as const;
// #4091 The backend always serves a list; tests seed it like the Rust default.
const SERVED = { contextHistorySessionWindows: [25, 100, 300], contextHistoryColorSessions: 300 };

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
    const snapshot = settingsSnapshot({ ...SERVED, ...overrides });
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

  it("resolves missing preferences to off, 30 days and no UI default list", async () => {
    const m = await renderModal({}, { omit: true });
    try {
      await waitFor(() => expect(m.$(ENABLED)).toBeTruthy());
      expect(m.$(TAB)!.getAttribute("data-ac-state")).toBe("active");
      expect(m.$<HTMLInputElement>(ENABLED)!.checked).toBe(false);
      expect(m.$<HTMLInputElement>(RETENTION)!.value).toBe("30");
      expect(m.$(RETENTION_ERROR)).toBeNull();
      expect(m.$<HTMLInputElement>(WINDOWS)!.value).toBe("");
      expect(m.$(WINDOWS_ERROR)).toBeTruthy();
      expect(m.$<HTMLSelectElement>(COLOR)!.options).toHaveLength(0);
    } finally { m.rendered.cleanup(); }
  });

  for (const sessions of [25, 100, 300]) {
    it(`restores saved values with a ${sessions}-session color window`, async () => {
      const m = await renderModal({
        contextHistoryEnabled: true,
        contextHistoryRetentionDays: 90,
        contextHistoryColorSessions: sessions,
      });
      try {
        await waitFor(() => expect(m.$<HTMLInputElement>(RETENTION)?.value).toBe("90"));
        expect(m.$<HTMLInputElement>(ENABLED)!.checked).toBe(true);
        expect(m.$<HTMLInputElement>(WINDOWS)!.value).toBe("25, 100, 300");
        const select = m.$<HTMLSelectElement>(COLOR)!;
        expect([...select.options].map((o) => o.textContent)).toEqual([
          "Últimas 25 sesiones", "Últimas 100 sesiones", "Últimas 300 sesiones",
        ]);
        expect(select.value).toBe(String(sessions));
      } finally { m.rendered.cleanup(); }
    });
  }

  it("a colour outside the list falls back to the last configured count", async () => {
    const m = await renderModal({
      contextHistorySessionWindows: [1, 10],
      contextHistoryColorSessions: 300,
    });
    try {
      await waitFor(() => expect(m.$(COLOR)).toBeTruthy());
      expect(m.$<HTMLSelectElement>(COLOR)!.value).toBe("10");
      expect(m.$<HTMLSelectElement>(COLOR)!.options[0].textContent).toBe("Última sesión");
    } finally { m.rendered.cleanup(); }
  });

  it("saves a custom list and colour, which reopen as saved", async () => {
    const m = await renderModal({ contextHistoryRetentionDays: 14 });
    try {
      await waitFor(() => expect(m.$(WINDOWS)).toBeTruthy());
      fire(m.$<HTMLInputElement>(WINDOWS)!, " 5, 50 ,500 ", "input");
      expect(m.$(WINDOWS_ERROR)).toBeNull();
      fire(m.$<HTMLSelectElement>(COLOR)!, "50", "change");
      const draft = await save(m);
      expect(draft.contextHistorySessionWindows).toEqual([5, 50, 500]);
      expect(draft.contextHistoryColorSessions).toBe(50);
      expect(draft.contextHistoryRetentionDays).toBe(14);
      m.rendered.cleanup();
      const again = await renderModal({
        contextHistorySessionWindows: draft.contextHistorySessionWindows,
        contextHistoryColorSessions: draft.contextHistoryColorSessions,
      });
      try {
        await waitFor(() => expect(again.$(WINDOWS)).toBeTruthy());
        expect(again.$<HTMLInputElement>(WINDOWS)!.value).toBe("5, 50, 500");
        expect(again.$<HTMLSelectElement>(COLOR)!.value).toBe("50");
      } finally { again.rendered.cleanup(); }
    } finally { m.rendered.cleanup(); }
  });

  it("removing the chosen colour count moves the colour to the last count", async () => {
    const m = await renderModal({ contextHistoryColorSessions: 100 });
    try {
      await waitFor(() => expect(m.$(WINDOWS)).toBeTruthy());
      fire(m.$<HTMLInputElement>(WINDOWS)!, "25, 300, 400", "input");
      expect(m.$<HTMLSelectElement>(COLOR)!.value).toBe("400");
      const draft = await save(m);
      expect(draft.contextHistorySessionWindows).toEqual([25, 300, 400]);
      expect(draft.contextHistoryColorSessions).toBe(400);
    } finally { m.rendered.cleanup(); }
  });

  it("keeps a colour count that stays in the edited list", async () => {
    const m = await renderModal({ contextHistoryColorSessions: 100 });
    try {
      await waitFor(() => expect(m.$(WINDOWS)).toBeTruthy());
      fire(m.$<HTMLInputElement>(WINDOWS)!, "100, 200", "input");
      expect((await save(m)).contextHistoryColorSessions).toBe(100);
    } finally { m.rendered.cleanup(); }
  });

  const BAD_LISTS = [
    "", " ", "25,", ",25", "25,,100", "abc", "0x10", "2.5", "1e2", "-1", "0",
    "25, 25", "100, 25", "1,2,3,4,5,6,7,8,9", "10001",
  ];
  for (const raw of BAD_LISTS) {
    it(`blocks Save for session windows ${JSON.stringify(raw)}; other drafts survive`, async () => {
      const m = await renderModal({ contextHistoryColorSessions: 100 });
      try {
        await waitFor(() => expect(m.$(WINDOWS)).toBeTruthy());
        fire(m.$<HTMLInputElement>(RETENTION)!, "21", "input");
        const input = m.$<HTMLInputElement>(WINDOWS)!;
        fire(input, raw, "input");
        await waitFor(() =>
          expect(m.$(WINDOWS_ERROR)?.textContent).toContain("Context Heat Map"),
        );
        const saveBtn = m.$<HTMLButtonElement>(SAVE)!;
        expect(saveBtn.disabled).toBe(true);
        saveBtn.click();
        expect(m.fake.lastCall("save_settings_draft")).toBeUndefined();
        expect(input.value).toBe(raw);
        expect(m.$<HTMLInputElement>(RETENTION)!.value).toBe("21");
        expect(m.$<HTMLSelectElement>(COLOR)!.value).toBe("100");
        fire(input, "25, 100, 300", "input");
        const draft = await save(m);
        expect(draft.contextHistorySessionWindows).toEqual([25, 100, 300]);
        expect(draft.contextHistoryRetentionDays).toBe(21);
      } finally { m.rendered.cleanup(); }
    });
  }

  it("accepts the 8-count and 10000 bounds", async () => {
    const m = await renderModal();
    try {
      await waitFor(() => expect(m.$(WINDOWS)).toBeTruthy());
      fire(m.$<HTMLInputElement>(WINDOWS)!, "1,2,3,4,5,6,7,10000", "input");
      expect(m.$(WINDOWS_ERROR)).toBeNull();
      expect((await save(m)).contextHistorySessionWindows).toEqual([1, 2, 3, 4, 5, 6, 7, 10000]);
    } finally { m.rendered.cleanup(); }
  });

  it("saves enable, numeric retention and color, keeping unrelated fields", async () => {
    const m = await renderModal(
      { contextHistoryColorSessions: 300, roomIdleConfirmationSeconds: 60 },
      { omit: false },
    );
    try {
      await waitFor(() => expect(m.$(ENABLED)).toBeTruthy());
      toggle(m.$<HTMLInputElement>(ENABLED)!);
      fire(m.$<HTMLInputElement>(RETENTION)!, "45", "input");
      fire(m.$<HTMLSelectElement>(COLOR)!, "100", "change");
      const draft = await save(m);
      expect(draft.contextHistoryEnabled).toBe(true);
      expect(draft.contextHistoryRetentionDays).toBe(45);
      expect(typeof draft.contextHistoryRetentionDays).toBe("number");
      expect(draft.contextHistoryColorSessions).toBe(100);
      expect(draft.contextHistorySessionWindows).toEqual([25, 100, 300]);
      expect(draft.roomIdleConfirmationSeconds).toBe(60);
    } finally { m.rendered.cleanup(); }
  });

  it("disabling saves false without touching retention or color", async () => {
    const m = await renderModal({
      contextHistoryEnabled: true,
      contextHistoryRetentionDays: 12,
      contextHistoryColorSessions: 25,
    });
    try {
      await waitFor(() => expect(m.$<HTMLInputElement>(ENABLED)?.checked).toBe(true));
      toggle(m.$<HTMLInputElement>(ENABLED)!);
      const draft = await save(m);
      expect(draft.contextHistoryEnabled).toBe(false);
      expect(draft.contextHistoryRetentionDays).toBe(12);
      expect(draft.contextHistoryColorSessions).toBe(25);
      expect(draft.contextHistorySessionWindows).toEqual([25, 100, 300]);
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
    const m = await renderModal({ contextHistoryEnabled: false });
    try {
      await waitFor(() => expect(m.$(ENABLED)).toBeTruthy());
      const before = { ...settingsStore.current };
      toggle(m.$<HTMLInputElement>(ENABLED)!);
      fire(m.$<HTMLInputElement>(RETENTION)!, "5", "input");
      fire(m.$<HTMLInputElement>(WINDOWS)!, "7, 70", "input");
      fire(m.$<HTMLSelectElement>(COLOR)!, "7", "change");
      for (const key of CTX_KEYS) expect(settingsStore.current?.[key]).toEqual(before[key]);
      m.$<HTMLButtonElement>(tid("settings.cancel"))!.click();
      expect(m.onClose).toHaveBeenCalledTimes(1);
      expect(m.fake.lastCall("save_settings_draft")).toBeUndefined();
    } finally { m.rendered.cleanup(); }
    const reopened = await renderModal({ ...settingsStore.current });
    try {
      await waitFor(() => expect(reopened.$(WINDOWS)).toBeTruthy());
      expect(reopened.$<HTMLInputElement>(WINDOWS)!.value).toBe("25, 100, 300");
      expect(reopened.$<HTMLSelectElement>(COLOR)!.value).toBe("300");
      expect(reopened.$<HTMLInputElement>(RETENTION)!.value).toBe("30");
    } finally { reopened.rendered.cleanup(); }
  });

  it("keeps the draft and the error after a failed Save", async () => {
    const m = await renderModal({ contextHistoryRetentionDays: 30 });
    try {
      await waitFor(() => expect(m.$(COLOR)).toBeTruthy());
      m.fake.reject("save_settings_draft", "color window write failed");
      const windows = m.$<HTMLInputElement>(WINDOWS)!;
      fire(windows, "10, 20", "input");
      const select = m.$<HTMLSelectElement>(COLOR)!;
      fire(select, "10", "change");
      m.$<HTMLButtonElement>(SAVE)!.click();
      await waitFor(() =>
        expect(m.rendered.root.querySelector(".modal-save-error")?.textContent)
          .toContain("color window write failed"),
      );
      expect(select.value).toBe("10");
      expect(windows.value).toBe("10, 20");
      expect(m.$<HTMLInputElement>(RETENTION)!.value).toBe("30");
      expect(m.onClose).not.toHaveBeenCalled();
    } finally { m.rendered.cleanup(); }
  });

  it("the browser hides the tab and opens General for a direct section request", async () => {
    platform.tauri = false;
    const m = await renderModal({ contextHistoryEnabled: true, contextHistoryColorSessions: 100 });
    try {
      await waitFor(() => expect(m.$(tid("settings.general.layout"))).toBeTruthy());
      expect(m.$(TAB)).toBeNull();
      expect(m.$(tid("settings.tab.general"))!.getAttribute("data-ac-state")).toBe("active");
      for (const sel of [ENABLED, RETENTION, WINDOWS, COLOR]) expect(m.$(sel)).toBeNull();
    } finally { m.rendered.cleanup(); }
  });
});
