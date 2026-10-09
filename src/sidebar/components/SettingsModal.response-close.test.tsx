// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { installBrowserDomStubs, renderWithFakeTransport, resetUiStoresForTests,
  settingsSnapshot, waitFor } from "../../shared/testing/ui-harness";
import type { AppSettings } from "../../shared/types";

const tid = (id: string) => `[data-ac-testid="settings.${id}"]`;
const FIELD = tid("general.responseCloseIdleSeconds");
const TOGGLE = tid("general.responseCloseEnabled");
const SAVE = tid("save");
const ERROR = tid("general.responseCloseIdleSeconds.error");

function type(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("SettingsModal response close (#2851)", () => {
  let cleanupDom: (() => void) | null = null;
  beforeEach(() => { cleanupDom = installBrowserDomStubs(); resetUiStoresForTests(); });
  afterEach(() => { cleanupDom?.(); resetUiStoresForTests(); document.body.replaceChildren(); });

  async function renderModal(overrides: Partial<AppSettings> = {}) {
    const fake = new FakeTransport();
    fake.resolve("get_settings", settingsSnapshot(overrides));
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    fake.resolve("save_settings_draft", undefined);
    fake.resolve("search_repos", []);
    let closed = 0;
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="general" onClose={() => { closed++; }} />, fake);
    await waitFor(() => expect(rendered.root.querySelector(FIELD)).toBeTruthy());
    const input = rendered.root.querySelector<HTMLInputElement>(FIELD)!;
    await waitFor(() => expect(input.value).toBe(String(overrides.responseCloseIdleSeconds ?? 30)));
    const toggle = rendered.root.querySelector<HTMLInputElement>(TOGGLE)!;
    const save = rendered.root.querySelector<HTMLButtonElement>(SAVE)!;
    return { fake, rendered, input, toggle, save, closed: () => closed };
  }

  it("defaults old snapshots to true/30 and exposes searchable labelled controls", async () => {
    const m = await renderModal();
    try {
      expect(m.toggle.checked).toBe(true);
      expect(m.toggle.closest("label")?.textContent).toContain("Close terminal after response");
      expect(m.input.closest("label")?.textContent).toContain("Idle seconds after response");
      expect([m.input.min, m.input.max, m.input.step]).toEqual(["1", "3600", "1"]);
      const search = m.rendered.root.querySelector<HTMLInputElement>(tid("general.search"))!;
      type(search, "idle seconds after response");
      await waitFor(() => expect(m.rendered.root.querySelector(".settings-general-result")).toBeTruthy());
      const result = m.rendered.root.querySelector<HTMLButtonElement>(".settings-general-result")!;
      expect(result.textContent).toContain("Idle seconds after response");
      result.click();
      expect(m.rendered.root.querySelector(tid("general.category.terminal"))?.getAttribute("aria-current")).toBe("page");
      m.save.click();
      await waitFor(() => expect(m.fake.lastCall("save_settings_draft")).toBeTruthy());
      const saved = (m.fake.lastCall("save_settings_draft")!.args as { draft: AppSettings }).draft;
      expect(saved.responseCloseEnabled).toBe(true);
      expect(saved.responseCloseIdleSeconds).toBe(30);
    } finally { m.rendered.cleanup(); }
  });

  it.each([1, 3600])("persists false and editable seconds %i, then reopens saved preferences", async (seconds) => {
    const m = await renderModal({ responseCloseIdleSeconds: 120 });
    let saved: AppSettings;
    try {
      m.toggle.click();
      expect(m.toggle.checked).toBe(false);
      expect(m.input.disabled).toBe(false);
      type(m.input, String(seconds));
      m.save.click();
      await waitFor(() => expect(m.fake.lastCall("save_settings_draft")).toBeTruthy());
      saved = (m.fake.lastCall("save_settings_draft")!.args as { draft: AppSettings }).draft;
      expect(saved.responseCloseEnabled).toBe(false);
      expect(saved.responseCloseIdleSeconds).toBe(seconds);
    } finally { m.rendered.cleanup(); }
    const reopened = await renderModal(saved!);
    try { expect(reopened.toggle.checked).toBe(false); expect(reopened.input.value).toBe(String(seconds)); }
    finally { reopened.rendered.cleanup(); }
  });

  it("blocks invalid text, marks the terminal category and never sends the last valid number", async () => {
    const m = await renderModal({ responseCloseIdleSeconds: 120 });
    try {
      for (const invalid of ["", "12.5", "-1", "0", "3601", "text", "9007199254740992"]) {
        type(m.input, invalid);
        expect(m.rendered.root.querySelector(ERROR)).toBeTruthy();
        expect(m.save.disabled).toBe(true);
        expect(m.rendered.root.querySelector(tid("general.category.terminal"))?.getAttribute("data-ac-state")).toBe("invalid");
        m.save.click();
        expect(m.fake.lastCall("save_settings_draft")).toBeFalsy();
      }
      type(m.input, "60");
      expect(m.save.disabled).toBe(false);
      expect(m.rendered.root.querySelector(ERROR)).toBeNull();
    } finally { m.rendered.cleanup(); }
  });

  it("cancels without persisting", async () => {
    const m = await renderModal();
    try {
      m.toggle.click(); type(m.input, "90");
      m.rendered.root.querySelector<HTMLButtonElement>(tid("cancel"))!.click();
      expect(m.closed()).toBe(1);
      expect(m.fake.lastCall("save_settings_draft")).toBeFalsy();
    } finally { m.rendered.cleanup(); }
  });

  it("retains explicit draft edits and the error when preferences fail to persist", async () => {
    const m = await renderModal({ responseCloseIdleSeconds: 120 });
    try {
      m.fake.reject("save_settings_draft", "preferences failed");
      m.toggle.click(); type(m.input, "90"); m.save.click();
      await waitFor(() => expect(m.rendered.root.querySelector(".modal-save-error")?.textContent).toContain("preferences failed"));
      expect(m.input.value).toBe("90"); expect(m.toggle.checked).toBe(false);
      expect(m.closed()).toBe(0);
    } finally { m.rendered.cleanup(); }
  });

  it.each(["terminal_snapshot_setting_conflict", "snapshot write failed"])(
    "keeps rebased committed preferences and synchronized text after later CAS failure: %s", async (error) => {
      const m = await renderModal({ responseCloseIdleSeconds: 120 });
      try {
        m.fake.resolve("get_settings", settingsSnapshot({ responseCloseEnabled: false, responseCloseIdleSeconds: 240,
          projectPath: "C:/fresh", projectPaths: ["C:/fresh"], archivedProjectPaths: ["C:/archive"] }));
        m.fake.reject("set_terminal_snapshots_enabled", error);
        m.rendered.root.querySelector<HTMLInputElement>(tid("general.terminalSnapshotsEnabled"))!.click();
        m.save.click();
        await waitFor(() => expect(m.rendered.root.querySelector(".modal-save-error")?.textContent).toBeTruthy());
        const saved = (m.fake.lastCall("save_settings_draft")!.args as { draft: AppSettings }).draft;
        expect(saved.responseCloseEnabled).toBe(false); expect(saved.responseCloseIdleSeconds).toBe(240);
        expect(saved.projectPath).toBe("C:/fresh"); expect(saved.projectPaths).toEqual(["C:/fresh"]);
        expect(saved.archivedProjectPaths).toEqual(["C:/archive"]);
        expect(m.input.value).toBe("240"); expect(m.toggle.checked).toBe(false);
        expect(m.rendered.root.querySelector(".modal-save-error")?.textContent).toContain(
          error === "terminal_snapshot_setting_conflict" ? "changed" : error);
        expect(m.closed()).toBe(0);
      } finally { m.rendered.cleanup(); }
    });

  it("preserves explicit preferences over fresh concurrent changes after later CAS failure", async () => {
    const m = await renderModal({ responseCloseIdleSeconds: 120 });
    try {
      m.toggle.click(); type(m.input, "90");
      m.fake.resolve("get_settings", settingsSnapshot({ responseCloseEnabled: true, responseCloseIdleSeconds: 240 }));
      m.fake.reject("set_terminal_snapshots_enabled", "snapshot write failed");
      m.rendered.root.querySelector<HTMLInputElement>(tid("general.terminalSnapshotsEnabled"))!.click(); m.save.click();
      await waitFor(() => expect(m.rendered.root.querySelector(".modal-save-error")?.textContent).toContain("snapshot write failed"));
      expect(m.input.value).toBe("90"); expect(m.toggle.checked).toBe(false);
      const saved = (m.fake.lastCall("save_settings_draft")!.args as { draft: AppSettings }).draft;
      expect(saved.responseCloseEnabled).toBe(false); expect(saved.responseCloseIdleSeconds).toBe(90);
    } finally { m.rendered.cleanup(); }
  });
});
