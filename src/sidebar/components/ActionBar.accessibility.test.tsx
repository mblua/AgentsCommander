// @vitest-environment jsdom
// #4945 characterization: ActionBar create notice and create confirmation dialogs.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component } from "solid-js";

// The platform seam flips the Tauri sentinel per test (same pattern as the CTX suite).
const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../../shared/platform", () => ({
  get isTauri() { return platform.tauri; },
  get isBrowser() { return !platform.tauri; },
  isWindows: false,
}));

import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { __disposeSettingsSubscriptionsForTests } from "../../shared/stores/settings";

// Kept in sync with the fixture plugin; the suite never imports the .mts.
const CONFIRM_PATH = "C:/issue-4945/fixture-project";

const sel = (id: string) => `[data-ac-testid="${id}"]`;
const q = <T extends HTMLElement = HTMLElement>(id: string) =>
  document.body.querySelector<T>(sel(id));
const must = <T extends HTMLElement = HTMLElement>(id: string): T => {
  const el = q<T>(id);
  if (!el) throw new Error(`Missing test target: ${id}`);
  return el;
};

const NOTICE = "project.browserCreateNotice.dialog";
const NOTICE_OVERLAY = "project.browserCreateNotice.overlay";
const CONFIRM = "project.createConfirm.dialog";
const CONFIRM_OVERLAY = "project.createConfirm.overlay";

function setupFake(): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_resource_snapshot", null);
  fake.resolve("pick_folder", CONFIRM_PATH);
  fake.resolve("check_project_path", false);
  fake.onInvoke("new_project", (args) => ({ path: args.path }));
  fake.resolve("discover_project", discovery());
  return fake;
}

let windowClicks: EventTarget[] = [];
const onWindowClick = (e: Event) => { windowClicks.push(e.target as EventTarget); };
const cleanups: Array<() => void> = [];

async function mount(Bar: Component) {
  const fake = setupFake();
  const rendered = renderWithFakeTransport(() => <Bar />, fake);
  cleanups.push(rendered.cleanup);
  await waitFor(() => expect(q("actionBar.settings")).toBeTruthy());
  return { fake, rendered };
}

function bubbleClick(el: Element): MouseEvent {
  const event = new MouseEvent("click", { bubbles: true, cancelable: true });
  el.dispatchEvent(event);
  return event;
}

function escape(el: Element): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
  el.dispatchEvent(event);
  return event;
}

async function openNewProject() {
  click(must("actionBar.newOpen"));
  await waitFor(() => expect(q("actionBar.menu.newProject")).toBeTruthy());
  click(must("actionBar.menu.newProject"));
}

beforeEach(() => {
  platform.tauri = true;
  cleanups.push(installBrowserDomStubs());
  resetUiStoresForTests();
  __disposeSettingsSubscriptionsForTests();
  windowClicks = [];
  window.addEventListener("click", onWindowClick);
});

afterEach(() => {
  window.removeEventListener("click", onWindowClick);
  while (cleanups.length) cleanups.pop()?.();
  resetUiStoresForTests();
  __disposeSettingsSubscriptionsForTests();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("ActionBar browser create notice (#4945 :471)", () => {
  async function openNotice() {
    platform.tauri = false;
    const { default: ActionBar } = await import("./ActionBar");
    const mounted = await mount(ActionBar);
    await openNewProject();
    await waitFor(() => expect(q(NOTICE)).toBeTruthy());
    windowClicks = [];
    return mounted;
  }

  it("browser create notice: opens a named modal dialog without creating", async () => {
    const { fake } = await openNotice();
    const dialog = must(NOTICE);
    expect(dialog.getAttribute("role")).toBe("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    const labelId = dialog.getAttribute("aria-labelledby");
    expect(labelId).toBe("projectBrowserCreateNoticeTitle");
    expect(document.getElementById(labelId!)?.textContent?.trim())
      .toBe("Create a project from the desktop app");
    expect(fake.callsFor("pick_folder")).toHaveLength(0);
    expect(fake.callsFor("new_project")).toHaveLength(0);
  });

  it("browser create notice: nested interior click is shielded from window", async () => {
    await openNotice();
    const paragraph = must(NOTICE).querySelector("p")!;
    bubbleClick(paragraph);
    bubbleClick(must(NOTICE));
    expect(windowClicks).toHaveLength(0);
    expect(q(NOTICE)).toBeTruthy();
  });

  it("browser create notice: bare overlay click is inert and reaches window", async () => {
    await openNotice();
    const overlay = must(NOTICE_OVERLAY);
    bubbleClick(overlay);
    expect(q(NOTICE)).toBeTruthy();
    // Baseline: the overlay has no click handler, so window sees the bare click.
    expect(windowClicks).toEqual([overlay]);
  });

  it("browser create notice: Escape keeps the notice open", async () => {
    await openNotice();
    escape(must(NOTICE));
    escape(document.body);
    await Promise.resolve();
    expect(q(NOTICE)).toBeTruthy();
  });

  it("browser create notice: Got it dismisses exactly once", async () => {
    await openNotice();
    bubbleClick(must("project.browserCreateNotice.dismiss"));
    await waitFor(() => expect(q(NOTICE)).toBeNull());
    expect(q(NOTICE_OVERLAY)).toBeNull();
    // Dismiss is inside the shielded dialog: window does not see it.
    expect(windowClicks).toHaveLength(0);
    await openNewProject();
    await waitFor(() => expect(q(NOTICE)).toBeTruthy());
    expect(document.body.querySelectorAll(sel(NOTICE))).toHaveLength(1);
  });
});

describe("ActionBar desktop create confirmation (#4945 :511)", () => {
  async function openConfirm() {
    platform.tauri = true;
    const { default: Variant } = await import("../components/ActionBar");
    const mounted = await mount(Variant);
    await waitFor(() => expect(q(CONFIRM)).toBeTruthy());
    windowClicks = [];
    return mounted;
  }

  it("desktop create confirmation: variant opens a modal dialog with the path", async () => {
    const { fake } = await openConfirm();
    const dialog = must(CONFIRM);
    expect(dialog.getAttribute("role")).toBe("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.querySelector(".confirm-path")?.textContent).toBe(CONFIRM_PATH);
    expect(fake.callsFor("new_project")).toHaveLength(0);
  });

  it("desktop create confirmation: No closes without creating", async () => {
    const { fake } = await openConfirm();
    bubbleClick(must("project.createConfirm.cancel"));
    await waitFor(() => expect(q(CONFIRM)).toBeNull());
    expect(windowClicks).toHaveLength(0);
    expect(fake.callsFor("new_project")).toHaveLength(0);
  });

  it("desktop create confirmation: Yes creates exactly once and closes", async () => {
    const { fake } = await openConfirm();
    bubbleClick(must("project.createConfirm.confirm"));
    await waitFor(() => expect(q(CONFIRM)).toBeNull());
    expect(windowClicks).toHaveLength(0);
    expect(fake.callsFor("new_project")).toEqual([
      { cmd: "new_project", args: { path: CONFIRM_PATH } },
    ]);
  });

  it("desktop create confirmation: Escape closes without creating", async () => {
    const { fake } = await openConfirm();
    escape(must(CONFIRM));
    await waitFor(() => expect(q(CONFIRM)).toBeNull());
    expect(fake.callsFor("new_project")).toHaveLength(0);
  });

  it("desktop create confirmation: nested click is shielded and stays open", async () => {
    const { fake } = await openConfirm();
    bubbleClick(must(CONFIRM).querySelector(".confirm-text")!);
    bubbleClick(must(CONFIRM));
    expect(windowClicks).toHaveLength(0);
    expect(q(CONFIRM)).toBeTruthy();
    expect(fake.callsFor("new_project")).toHaveLength(0);
  });

  it("desktop create confirmation: bare overlay click is inert", async () => {
    const { fake } = await openConfirm();
    const overlay = must(CONFIRM_OVERLAY);
    bubbleClick(overlay);
    expect(q(CONFIRM)).toBeTruthy();
    // Baseline: the overlay has no click handler, so window sees the bare click.
    expect(windowClicks).toEqual([overlay]);
    expect(fake.callsFor("new_project")).toHaveLength(0);
  });
});

describe("ActionBar ordinary desktop New Project (#4945 P3)", () => {
  it("variant and ordinary modules are distinct", async () => {
    const ordinary = await import("./ActionBar");
    const variant = await import("../components/ActionBar");
    expect(variant.default).not.toBe(ordinary.default);
  });

  it("New Project directly calls createAndLoad without a confirmation", async () => {
    platform.tauri = true;
    const { default: ActionBar } = await import("./ActionBar");
    const { fake } = await mount(ActionBar);
    expect(q(CONFIRM)).toBeNull();
    await openNewProject();
    await waitFor(() => expect(fake.callsFor("new_project")).toHaveLength(1));
    expect(fake.lastCall("new_project")?.args).toEqual({ path: CONFIRM_PATH });
    expect(fake.callsFor("pick_folder")).toHaveLength(1);
    expect(q(CONFIRM)).toBeNull();
    expect(q(NOTICE)).toBeNull();
  });
});
