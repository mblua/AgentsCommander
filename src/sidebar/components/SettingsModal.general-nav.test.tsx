// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  settingsSnapshot,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { AppSettings } from "../../shared/types";
import {
  GENERAL_CATEGORIES,
  GENERAL_SETTINGS_INDEX,
  searchGeneralSettings,
} from "./settings/generalSettingsIndex";
import { declValue, declarations, scanRules } from "../styles/css-test-helpers";

// #2704 - Settings > General is split into categories with a cross-category search.

// Vite rewrites the literal `new URL(..., import.meta.url)` form into a served
// asset; a binding keeps the file: URL that node:fs accepts.
const moduleUrl = import.meta.url;
const tid = (id: string) => `[data-ac-testid="${id}"]`;
const LAYOUT = tid("settings.general.layout");
const SEARCH = tid("settings.general.search");
const SAVE = tid("settings.save");
const pane = (id: string) => tid(`settings.general.pane.${id}`);
const cat = (id: string) => tid(`settings.general.category.${id}`);

// Every always-rendered General testid on main (2bd7242c) before this change.
const PRE_EXISTING_TESTIDS = [
  "settings.apiClientMint.expiry",
  "settings.apiClientMint.label",
  "settings.apiClientMint.root",
  "settings.apiClientMint.submit",
  "settings.apiClientMint.surface",
  "settings.apiClientMint.scope.send",
  "settings.apiClientMint.scope.list-peers-lean",
  "settings.apiClientMint.scope.session-transport",
  "settings.general.activityLogEnabled",
  "settings.general.activityLogEnabled.hint",
  "settings.general.apiServerBind",
  "settings.general.apiServerEnabled",
  "settings.general.apiServerPort",
  "settings.general.apiServerStatus",
  "settings.general.autoSelfClearEnabled",
  "settings.general.containerCredentialsFromHost",
  "settings.general.containerCredentialsFromHost.hint",
  "settings.general.coordinatorAutoCloseEnabled",
  "settings.general.coordinatorAutoCloseMinutes",
  "settings.general.coordinatorAutoCloseSkipTelegramAssigned",
  "settings.general.coordinatorCascadeCloseEnabled",
  "settings.general.coordinatorIdleBadgeRedMinutes",
  "settings.general.coordinatorIdleBadgeYellowMinutes",
  "settings.general.defaultShell",
  "settings.general.logLevel",
  "settings.general.npmUpdateNotificationsEnabled",
  "settings.general.remoteBlockingMenusEnabled",
  "settings.general.restartResumeAgentPrompt",
  "settings.general.restartResumeOrchestratorPrompt",
  "settings.general.restartResumeWakeWorkingAgents",
  "settings.general.restoreCoordinatorWakeState",
  "settings.general.roomNumberMask",
  "settings.general.roomNumberMask.example",
  "settings.general.screenshotCaptureHotkey",
  "settings.general.selectedRowRailColor",
  "settings.general.selectedRowRailWidth",
  "settings.general.sidebarCompactHotkey",
  "settings.general.terminalSnapshotsEnabled",
  "settings.general.terminalSnapshotsEnabled.warning",
  "settings.general.typingHoldSeconds",
];

describe("SettingsModal General categories + search (#2704)", () => {
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

  async function renderModal(overrides: Partial<AppSettings> = {}) {
    const fake = new FakeTransport();
    fake.resolve("get_settings", settingsSnapshot(overrides));
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    fake.resolve("save_settings_draft", undefined);
    const onClose = vi.fn();
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="general" onClose={onClose} />,
      fake,
    );
    await waitFor(() => expect(rendered.root.querySelector(LAYOUT)).toBeTruthy());
    const $ = <T extends Element = HTMLElement>(sel: string) => rendered.root.querySelector<T>(sel)!;
    return { fake, rendered, onClose, $ };
  }

  function type(input: HTMLInputElement | HTMLSelectElement, value: string, event = "input") {
    input.value = value;
    input.dispatchEvent(new Event(event, { bubbles: true }));
  }

  function key(el: Element, k: string) {
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  }

  it("opens on Appearance with the other panes hidden", async () => {
    const { rendered, $ } = await renderModal();
    try {
      expect($(pane("appearance")).hidden).toBe(false);
      for (const c of GENERAL_CATEGORIES.slice(1)) expect($(pane(c.id)).hidden).toBe(true);
      expect($(cat("appearance")).getAttribute("aria-current")).toBe("page");
      expect(
        Array.from(rendered.root.querySelectorAll(".settings-general-cat")).map((b) =>
          b.querySelector(".settings-general-cat-label")!.textContent,
        ),
      ).toEqual(["Appearance", "Terminal", "Agents", "Network & remote access", "System"]);
    } finally {
      rendered.cleanup();
    }
  });

  it("switches category and keeps edits made in another pane", async () => {
    const { rendered, $ } = await renderModal();
    try {
      const mask = $<HTMLInputElement>(tid("settings.general.roomNumberMask"));
      type(mask, "R-##");
      $<HTMLButtonElement>(cat("system")).click();
      expect($(pane("system")).hidden).toBe(false);
      expect($(pane("appearance")).hidden).toBe(true);
      expect($(cat("system")).getAttribute("aria-current")).toBe("page");
      $<HTMLButtonElement>(cat("appearance")).click();
      expect($<HTMLInputElement>(tid("settings.general.roomNumberMask")).value).toBe("R-##");
    } finally {
      rendered.cleanup();
    }
  });

  it("indexes every rendered General control exactly once", async () => {
    const { rendered, $ } = await renderModal({ apiServerEnabled: true, webServerEnabled: true });
    try {
      await waitFor(() => expect($(tid("settings.general.apiServerEnabled"))).toBeTruthy());
      const indexKeys = GENERAL_SETTINGS_INDEX.map((e) => e.key);
      const domKeys = Array.from(rendered.root.querySelectorAll("[data-ac-setting]")).map(
        (el) => el.getAttribute("data-ac-setting")!,
      );
      expect(new Set(domKeys).size).toBe(domKeys.length);
      expect([...domKeys].sort()).toEqual([...indexKeys].sort());

      // 3b - every form control sits under an indexed wrapper.
      const layout = $(LAYOUT);
      for (const control of Array.from(layout.querySelectorAll("input, select, textarea"))) {
        if (control.matches(SEARCH)) continue;
        const wrapper = control.closest("[data-ac-setting]");
        expect(wrapper, control.outerHTML).toBeTruthy();
        expect(indexKeys).toContain(wrapper!.getAttribute("data-ac-setting"));
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("puts the rail fields in Appearance and opens them from search", async () => {
    const { rendered, $ } = await renderModal();
    try {
      for (const k of ["selectedRowRailWidth", "selectedRowRailColor"]) {
        expect($(pane("appearance")).querySelector(`[data-ac-setting="${k}"]`)).toBeTruthy();
      }
      expect(searchGeneralSettings("bar color").map((e) => e.key)).toContain("selectedRowRailColor");
      type($<HTMLInputElement>(SEARCH), "bar color");
      $<HTMLButtonElement>(tid("settings.general.result.selectedRowRailColor")).click();
      await waitFor(() =>
        expect(document.activeElement).toBe($(tid("settings.general.selectedRowRailColor"))),
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("indexes the scope checkboxes as one Scopes entry", async () => {
    const { rendered, $ } = await renderModal({ apiServerEnabled: true });
    try {
      const scopeEntries = GENERAL_SETTINGS_INDEX.filter((e) => e.key.startsWith("apiClientMint") && /scope/i.test(e.key));
      expect(scopeEntries.map((e) => e.key)).toEqual(["apiClientMintScopes"]);
      expect(searchGeneralSettings("session-transport").map((e) => e.key)).toEqual(["apiClientMintScopes"]);
      const wrappers = rendered.root.querySelectorAll('[data-ac-setting="apiClientMintScopes"]');
      expect(wrappers.length).toBe(1);
      expect(wrappers[0].querySelectorAll('input[type="checkbox"]').length).toBe(3);
      type($<HTMLInputElement>(SEARCH), "session-transport");
      $<HTMLButtonElement>(tid("settings.general.result.apiClientMintScopes")).click();
      await waitFor(() =>
        expect(document.activeElement).toBe($(tid("settings.apiClientMint.scope.send"))),
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("marks the category that holds a blocking error with a red dot", async () => {
    const { rendered, $ } = await renderModal({ typingHoldSeconds: 120 });
    try {
      const input = $<HTMLInputElement>(tid("settings.general.typingHoldSeconds"));
      await waitFor(() => expect(input.value).toBe("120"));
      $<HTMLButtonElement>(cat("terminal")).click();
      type(input, "0");
      $<HTMLButtonElement>(cat("appearance")).click();
      await waitFor(() => expect($(cat("terminal")).getAttribute("data-ac-state")).toBe("invalid"));
      expect($(cat("terminal")).querySelector(".settings-general-cat-invalid-dot")).toBeTruthy();
      expect($(cat("terminal")).textContent).toContain("has an invalid value");
      for (const c of GENERAL_CATEGORIES.filter((c) => c.id !== "terminal")) {
        expect($(cat(c.id)).hasAttribute("data-ac-state")).toBe(false);
        expect($(cat(c.id)).querySelector(".settings-general-cat-invalid-dot")).toBeNull();
      }
      expect($<HTMLButtonElement>(SAVE).disabled).toBe(true);
      expect(rendered.root.querySelector(".modal-save-error")?.textContent).toContain("Typing hold");

      type(input, "60");
      await waitFor(() => expect($(cat("terminal")).hasAttribute("data-ac-state")).toBe(false));
      expect($(cat("terminal")).querySelector(".settings-general-cat-invalid-dot")).toBeNull();
      expect($<HTMLButtonElement>(SAVE).disabled).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps every pre-existing General testid in the DOM", async () => {
    const { rendered } = await renderModal();
    try {
      for (const id of PRE_EXISTING_TESTIDS) {
        expect(rendered.root.querySelector(tid(id)), id).toBeTruthy();
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("searches across categories with per-category counts", async () => {
    const { rendered, $ } = await renderModal();
    try {
      type($<HTMLInputElement>(SEARCH), "hotkey");
      expect(rendered.root.querySelectorAll(".settings-general-result").length).toBe(2);
      for (const c of GENERAL_CATEGORIES) expect($(pane(c.id)).hidden).toBe(true);
      const counts = GENERAL_CATEGORIES.map(
        (c) => $(cat(c.id)).querySelector(".settings-general-cat-count")!.textContent,
      );
      expect(counts).toEqual(["2", "0", "0", "0", "0"]);
      expect($(cat("terminal")).classList.contains("is-empty")).toBe(true);
      expect($(cat("appearance")).hasAttribute("aria-current")).toBe(false);
      expect($(".settings-general-result-path").textContent).toBe("Appearance › Hotkeys");
    } finally {
      rendered.cleanup();
    }
  });

  it("opens and focuses a result, clearing the search", async () => {
    const { rendered, $ } = await renderModal();
    try {
      $<HTMLButtonElement>(cat("system")).click();
      type($<HTMLInputElement>(SEARCH), "hotkey");
      $<HTMLButtonElement>(tid("settings.general.result.sidebarCompactHotkey")).click();
      expect($<HTMLInputElement>(SEARCH).value).toBe("");
      expect($(pane("appearance")).hidden).toBe(false);
      await waitFor(() =>
        expect(document.activeElement).toBe($(tid("settings.general.sidebarCompactHotkey"))),
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the empty state when nothing matches", async () => {
    const { rendered, $ } = await renderModal();
    try {
      type($<HTMLInputElement>(SEARCH), "zzqx");
      expect($(tid("settings.general.search.empty")).textContent).toContain("zzqx");
      expect(rendered.root.querySelector(".settings-general-result-count")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("Escape clears a non-empty search and only closes the modal when empty", async () => {
    const { rendered, onClose, $ } = await renderModal();
    try {
      const search = $<HTMLInputElement>(SEARCH);
      type(search, "port");
      key(search, "Escape");
      expect(search.value).toBe("");
      expect(onClose).not.toHaveBeenCalled();
      key(search, "Escape");
      expect(onClose).toHaveBeenCalledTimes(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("Enter opens the first result; arrows move between results", async () => {
    const { rendered, $ } = await renderModal();
    try {
      const search = $<HTMLInputElement>(SEARCH);
      type(search, "hotkey");
      search.focus();
      key(search, "ArrowDown");
      const results = rendered.root.querySelectorAll<HTMLButtonElement>(".settings-general-result");
      expect(document.activeElement).toBe(results[0]);
      key(results[0], "ArrowDown");
      expect(document.activeElement).toBe(results[1]);
      key(results[1], "ArrowUp");
      key(results[0], "ArrowUp");
      expect(document.activeElement).toBe(search);

      key(search, "Enter");
      expect(search.value).toBe("");
      await waitFor(() =>
        expect(document.activeElement).toBe($(tid("settings.general.screenshotCaptureHotkey"))),
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("saves edits made in different categories", async () => {
    const { fake, rendered, $ } = await renderModal({ defaultShell: "/bin/bash", logLevel: "info" });
    try {
      const shell = $<HTMLInputElement>(tid("settings.general.defaultShell"));
      await waitFor(() => expect(shell.value).toBe("/bin/bash"));
      $<HTMLButtonElement>(cat("terminal")).click();
      type(shell, "/bin/zsh");
      $<HTMLButtonElement>(cat("system")).click();
      type($<HTMLSelectElement>(tid("settings.general.logLevel")), "debug", "change");
      $<HTMLButtonElement>(SAVE).click();
      await waitFor(() => expect(fake.lastCall("save_settings_draft")).toBeTruthy());
      const saved = (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings }).draft;
      expect(saved.defaultShell).toBe("/bin/zsh");
      expect(saved.logLevel).toBe("debug");
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps category and query across top-tab switches", async () => {
    const { rendered, $ } = await renderModal();
    try {
      $<HTMLButtonElement>(cat("network")).click();
      $<HTMLButtonElement>(tid("settings.tab.agents")).click();
      await waitFor(() => expect(rendered.root.querySelector(LAYOUT)).toBeNull());
      $<HTMLButtonElement>(tid("settings.tab.general")).click();
      await waitFor(() => expect(rendered.root.querySelector(LAYOUT)).toBeTruthy());
      expect($(pane("network")).hidden).toBe(false);

      type($<HTMLInputElement>(SEARCH), "log");
      $<HTMLButtonElement>(tid("settings.tab.agents")).click();
      $<HTMLButtonElement>(tid("settings.tab.general")).click();
      await waitFor(() => expect(rendered.root.querySelector(LAYOUT)).toBeTruthy());
      expect($<HTMLInputElement>(SEARCH).value).toBe("log");
      expect(rendered.root.querySelectorAll(".settings-general-result").length).toBeGreaterThan(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("widens the modal only on the General tab", async () => {
    const { rendered, $ } = await renderModal();
    try {
      const modal = $(tid("settings.modal"));
      expect(modal.classList.contains("modal-container-general")).toBe(true);
      $<HTMLButtonElement>(tid("settings.tab.agents")).click();
      expect(modal.classList.contains("modal-container-general")).toBe(false);
      expect(modal.classList.contains("modal-container-config")).toBe(true);
      for (const tab of ["resources", "watchers", "integrations"]) {
        const btn = rendered.root.querySelector<HTMLButtonElement>(tid(`settings.tab.${tab}`));
        if (!btn) continue;
        btn.click();
        expect(modal.classList.contains("modal-container-general")).toBe(false);
      }
    } finally {
      rendered.cleanup();
    }
  });

  // Round 4: at <=760px the categories wrap as chips. The bytes come from disk
  // because a CSS ?raw import evaluates to "" under Vitest (see agent-badge-css.test.ts).
  it("stacks the categories as wrapping chips at 760px and keeps the red dot on the chip", async () => {
    const css = readFileSync(new URL("../styles/sidebar.css", moduleUrl), "utf8").replace(
      /\/\*[\s\S]*?\*\//g,
      "",
    );
    const mediaStart = css.indexOf("@media (max-width: 760px)");
    expect(mediaStart).toBeGreaterThan(-1);
    let depth = 0;
    let end = -1;
    for (let i = css.indexOf("{", mediaStart); i < css.length; i++) {
      if (css[i] === "{") depth++;
      else if (css[i] === "}" && --depth === 0) {
        end = i;
        break;
      }
    }
    const rules = scanRules(css.slice(css.indexOf("{", mediaStart) + 1, end));
    const rule = (sel: string) => {
      const found = rules.find((r) => r.selectors.includes(sel));
      if (!found) throw new Error(`missing rule in 760px block: ${sel}`);
      return found.body;
    };
    const nav = rule(".settings-general-nav");
    expect(declValue(nav, "flex-wrap")).toBe("wrap");
    expect(declValue(nav, "flex-direction")).toBe("row");
    for (const [p, v] of declarations(nav)) if (p === "max-height") expect(v).toBe("none");
    expect(declValue(rule(".settings-general-cat"), "width")).toBe("auto");

    const { rendered, $ } = await renderModal({ typingHoldSeconds: 120 });
    try {
      const input = $<HTMLInputElement>(tid("settings.general.typingHoldSeconds"));
      await waitFor(() => expect(input.value).toBe("120"));
      $<HTMLButtonElement>(cat("terminal")).click();
      type(input, "0");
      $<HTMLButtonElement>(cat("appearance")).click();
      await waitFor(() => expect($(cat("terminal")).getAttribute("data-ac-state")).toBe("invalid"));
      const dot = rendered.root.querySelector(".settings-general-cat-invalid-dot");
      expect(dot).toBeTruthy();
      expect($(cat("terminal")).contains(dot)).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });
});
