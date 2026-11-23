// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component } from "solid-js";
import type { WebServerOwnedStatus } from "../../shared/types";
import type { FakeTransport } from "../../shared/testing/fake-transport";

// #4945 characterization of the WebServer popover's accessibility contract,
// mounted through Titlebar with the fixtures of Titlebar.webserver.test.tsx.

const tauriStubs = vi.hoisted(() => {
  const resolved = <T,>(value: T) => () => Promise.resolve(value);
  return {
    core: { invoke: resolved("") },
    win: {
      close: () => undefined,
      maximize: () => undefined,
      minimize: () => undefined,
      unmaximize: () => undefined,
      isMaximized: resolved(false),
    },
  };
});
vi.mock("@tauri-apps/api/core", () => tauriStubs.core);
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => tauriStubs.win }));

type Testing = typeof import("../../shared/testing/ui-harness");
interface HarnessModules {
  Titlebar: Component;
  FakeTransport: typeof FakeTransport;
  baseSettings: Testing["baseSettings"];
  renderWithFakeTransport: Testing["renderWithFakeTransport"];
  waitFor: Testing["waitFor"];
}

const cleanups: Array<() => void> = [];

const stoppedStatus = (): WebServerOwnedStatus => ({
  listening: false,
  owned: false,
  externalListening: false,
  openAllowed: false,
  bind: "127.0.0.1",
  port: 8765,
  state: "stopped",
  bindFailure: null,
});

async function mountTitlebar(): Promise<HarnessModules> {
  const [{ default: Titlebar }, testing, fakeTransport] = await Promise.all([
    import("./Titlebar"),
    import("../../shared/testing/ui-harness"),
    import("../../shared/testing/fake-transport"),
  ]);
  const modules: HarnessModules = {
    Titlebar,
    FakeTransport: fakeTransport.FakeTransport,
    baseSettings: testing.baseSettings,
    renderWithFakeTransport: testing.renderWithFakeTransport,
    waitFor: testing.waitFor,
  };
  const fake = new modules.FakeTransport();
  const settings = modules.baseSettings({ webServerEnabled: false, webServerBind: "127.0.0.1" });
  fake.onInvoke("get_settings", () => settings);
  fake.onInvoke("save_settings_draft", () => undefined);
  fake.onInvoke("get_web_server_owned_status", () => stoppedStatus());
  fake.onInvoke("start_web_server", () => true);
  fake.onInvoke("stop_web_server", () => true);
  fake.onInvoke("open_web_remote", () => undefined);
  fake.onInvoke("list_web_server_interfaces", () => [
    { address: "192.168.1.9", interfaceName: "Ethernet", isVirtual: false },
  ]);
  const rendered = modules.renderWithFakeTransport(() => <modules.Titlebar />, fake);
  cleanups.push(rendered.cleanup);
  return modules;
}

function maybeByTestId<T extends Element = Element>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function byTestId<T extends Element = Element>(testId: string): T {
  const element = maybeByTestId<T>(testId);
  if (!element) throw new Error(`missing selector ${testId}`);
  return element;
}

const click = (el: Element) =>
  el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));

const pressEscape = (el: Element) =>
  el.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
  );

// Name computed from aria-labelledby, aria-label, then subtree text; the
// title (tooltip) fallback is deliberately excluded.
function nameWithoutTooltip(el: Element): string {
  const labelledBy = el.getAttribute("aria-labelledby");
  if (labelledBy) {
    return labelledBy
      .split(/\s+/)
      .map((id) => document.getElementById(id)?.textContent ?? "")
      .join(" ")
      .trim();
  }
  const label = el.getAttribute("aria-label")?.trim();
  if (label) return label;
  return (el.textContent ?? "").trim();
}

async function openMenu(modules: HarnessModules): Promise<HTMLElement> {
  click(byTestId("titlebar.webserver.button"));
  await modules.waitFor(() => expect(maybeByTestId("titlebar.webserver.menu")).toBeTruthy());
  return byTestId<HTMLElement>("titlebar.webserver.menu");
}

async function enterEditor(modules: HarnessModules): Promise<HTMLInputElement> {
  click(byTestId("titlebar.webserver.editAddr"));
  await modules.waitFor(() => expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeTruthy());
  const input = byTestId<HTMLInputElement>("titlebar.webserver.addrInput");
  input.focus();
  expect(document.activeElement).toBe(input);
  return input;
}

describe("WebServerMenu accessibility (#4945)", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.resetModules();
    vi.clearAllMocks();
  });

  afterEach(() => {
    while (cleanups.length > 0) cleanups.pop()?.();
    document.body.innerHTML = "";
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    vi.clearAllMocks();
  });

  it("1. the trigger is a native button whose name does not depend on its tooltip", async () => {
    const modules = await mountTitlebar();
    const trigger = byTestId<HTMLButtonElement>("titlebar.webserver.button");
    await modules.waitFor(() => expect(trigger.getAttribute("title")).toBeTruthy());
    expect(trigger.tagName).toBe("BUTTON");
    // Current baseline: no aria-label; the name comes from the globe glyph.
    expect(trigger.hasAttribute("aria-label")).toBe(false);
    const withTitle = nameWithoutTooltip(trigger);
    expect(withTitle).toBe("\u{1F310}");
    trigger.removeAttribute("title");
    expect(nameWithoutTooltip(trigger)).toBe(withTitle);
  });

  it("2. opening the dropdown and entering the address editor moves real focus there", async () => {
    const modules = await mountTitlebar();
    const menu = await openMenu(modules);
    const input = await enterEditor(modules);
    expect(menu.contains(document.activeElement)).toBe(true);
    expect(input.isConnected).toBe(true);
  });

  it("3. P6: Escape in the editor exits editing; a second Escape with no refocus", async () => {
    const modules = await mountTitlebar();
    await openMenu(modules);
    const input = await enterEditor(modules);

    const first = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    input.dispatchEvent(first);
    await modules.waitFor(() => expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeNull());
    expect(maybeByTestId("titlebar.webserver.menu")).toBeTruthy();
    expect(input.isConnected).toBe(false);
    // Observed: the editor teardown drops focus to <body>; nothing refocuses.
    const afterTeardown = document.activeElement;
    expect(afterTeardown).toBe(document.body);

    // Second Escape from wherever focus now is, without refocusing.
    pressEscape(afterTeardown ?? document.body);
    await Promise.resolve();
    await Promise.resolve();
    // Observed: the element-bound listener never sees it, so the popover persists.
    expect(maybeByTestId("titlebar.webserver.menu")).toBeTruthy();
    expect(document.activeElement).toBe(afterTeardown);
  });

  it("4. on a separate open, Escape from the surviving Edit button closes it", async () => {
    const modules = await mountTitlebar();
    await openMenu(modules);
    click(byTestId("titlebar.webserver.button"));
    await modules.waitFor(() => expect(maybeByTestId("titlebar.webserver.menu")).toBeNull());

    await openMenu(modules);
    const edit = byTestId<HTMLButtonElement>("titlebar.webserver.editAddr");
    edit.focus();
    expect(document.activeElement).toBe(edit);
    expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeNull();
    pressEscape(edit);
    await modules.waitFor(() => expect(maybeByTestId("titlebar.webserver.menu")).toBeNull());
  });

  it("5. an interior click keeps it open with zero window bubble-phase clicks", async () => {
    const modules = await mountTitlebar();
    const menu = await openMenu(modules);
    let windowClicks = 0;
    const onWindowClick = () => windowClicks++;
    window.addEventListener("click", onWindowClick);
    try {
      click(menu.querySelector(".webserver-url") ?? menu);
      click(menu);
      await Promise.resolve();
      expect(windowClicks).toBe(0);
      expect(maybeByTestId("titlebar.webserver.menu")).toBe(menu);
    } finally {
      window.removeEventListener("click", onWindowClick);
    }
  });

  it("6. an outside click dismisses the dropdown", async () => {
    const modules = await mountTitlebar();
    await openMenu(modules);
    const outside = document.createElement("div");
    document.body.appendChild(outside);
    let windowClicks = 0;
    const onWindowClick = () => windowClicks++;
    window.addEventListener("click", onWindowClick);
    try {
      click(outside);
      // Positive control for case 5: an outside click does reach window.
      expect(windowClicks).toBe(1);
    } finally {
      window.removeEventListener("click", onWindowClick);
    }
    await modules.waitFor(() => expect(maybeByTestId("titlebar.webserver.menu")).toBeNull());
  });
});
