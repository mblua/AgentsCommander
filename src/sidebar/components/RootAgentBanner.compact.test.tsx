// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JSX } from "solid-js";
import RootAgentBanner from "./RootAgentBanner";
import SidebarApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  registerCompactHostForTests,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import {
  currentHotkey,
  DEFAULT_SIDEBAR_COMPACT_HOTKEY,
  setSidebarCompactHotkey,
  setSidebarCompactMode,
  sidebarCompact,
} from "../../shared/sidebar-compact";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder } from "../../shared/voice-recorder";
import { declValue, scanRules } from "../styles/css-test-helpers";
import { initialSelection } from "../../shared/testing/session-selection";

// #2236 phase 9 (#2284) - the banner's collapse/expand toggle, the row's real
// activation button and the hotkey hydration. jsdom applies no stylesheet, so
// hit-testing claims are read from sidebar.css as text, never from a click.

// Vite rewrites the literal `new URL(..., import.meta.url)` form into a served
// asset; reading import.meta.url through a binding keeps the file: URL that
// node:fs accepts (AgentUpdateOverlay.test.tsx).
const moduleUrl = import.meta.url;

const BANNER_SEL = '[data-ac-testid="rootAgent.banner"]';

function rootLive(overrides: Partial<ReturnType<typeof session>> = {}) {
  return session({ id: "root", name: "Root", isRootAgent: true, status: "running", ...overrides });
}

function bannerFake(): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("switch_session", undefined);
  return fake;
}

// Every render is released in afterEach, so no test needs its own try/finally.
const mounted: Array<() => void> = [];
function mount(component: () => JSX.Element, fake: FakeTransport) {
  const rendered = renderWithFakeTransport(component, fake);
  mounted.push(rendered.cleanup);
  return rendered;
}

function renderBanner(fake = bannerFake()) {
  const rendered = mount(() => <RootAgentBanner />, fake);
  const banner = rendered.root.querySelector(BANNER_SEL) as HTMLElement;
  const toggle = banner.querySelector(".root-agent-banner-toggle") as HTMLButtonElement;
  const open = banner.querySelector(".root-agent-banner-open") as HTMLButtonElement;
  return { ...rendered, banner, toggle, open };
}

const selections = (fake: FakeTransport) => fake.callsFor("switch_session").length;

function setupApp(fake: FakeTransport, settings = baseSettings({ projectPaths: [], projectPath: null })) {
  fake.resolve("get_settings", settings);
  fake.resolve("get_update_status", null);
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", []);
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("drain_session_warnings", []);
  fake.resolve("get_active_session", initialSelection());
  fake.resolve("screenshot_get_hotkey_status", { configured: "Ctrl+Q", registered: true, error: null });
}

// The last startup invoke; unmounting before it lets a late call reach the
// default (WebSocket) transport after the fake is restored.
const appSettled = (fake: FakeTransport) =>
  waitFor(() => expect(fake.callsFor("get_agent_update_status").length).toBeGreaterThan(0));

describe("RootAgentBanner compact toggle (#2284)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    registerCompactHostForTests();
  });

  afterEach(() => {
    while (mounted.length > 0) mounted.pop()?.();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    vi.restoreAllMocks();
  });

  it("1. expanded: the toggle is the last child, shows >> and aria-expanded=true", () => {
    const r = renderBanner();
    expect(r.banner.lastElementChild).toBe(r.toggle);
    expect(r.toggle.textContent).toBe(">>");
    expect(r.toggle.getAttribute("aria-expanded")).toBe("true");
  });

  it("2. compact: the toggle shows << and aria-expanded=false", () => {
    setSidebarCompactMode(true);
    const r = renderBanner();
    expect(r.toggle.textContent).toBe("<<");
    expect(r.toggle.getAttribute("aria-expanded")).toBe("false");
  });

  it("3. clicking the toggle toggles compact and does not select the Root Agent", async () => {
    sessionsStore.setSessions([rootLive()]);
    const r = renderBanner();
    r.toggle.click();
    expect(sidebarCompact()).toBe(true);
    expect(r.toggle.textContent).toBe("<<");
    r.toggle.click();
    expect(sidebarCompact()).toBe(false);
    await Promise.resolve();
    expect(selections(r.fake)).toBe(0);
  });

  it("4. the open button selects while expanded; compact hides it by stylesheet and the toggle never selects", async () => {
    sessionsStore.setSessions([rootLive()]);
    const r = renderBanner();
    r.open.click();
    await waitFor(() => expect(selections(r.fake)).toBe(1));

    const css = readFileSync(new URL("../styles/sidebar.css", moduleUrl), "utf8");
    const rules = scanRules(css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " ")));
    const hidden = ".sidebar-layout.sidebar-compact .root-agent-banner > :not(.root-agent-banner-toggle)";
    const rule = rules.find((x) => x.selectors.includes(hidden));
    expect(rule, hidden).toBeDefined();
    expect(declValue(rule!.body, "visibility")).toBe("hidden");
    // The open button is a direct child and not the toggle, so the complement matches it.
    expect(r.open.parentElement).toBe(r.banner);
    expect(r.open.matches(":not(.root-agent-banner-toggle)")).toBe(true);

    setSidebarCompactMode(true);
    r.toggle.click();
    await Promise.resolve();
    expect(selections(r.fake)).toBe(1);
  });

  it("4a. #2519: compact keeps the expanded row height (hidden text never wraps, banner clips)", () => {
    // jsdom has no layout, so the height itself cannot be measured; pin the two rules that hold it.
    const css = readFileSync(new URL("../styles/sidebar.css", moduleUrl), "utf8");
    const rules = scanRules(css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " ")));
    const hidden = ".sidebar-layout.sidebar-compact .root-agent-banner > :not(.root-agent-banner-toggle)";
    const hiddenRule = rules.find((x) => x.selectors.includes(hidden));
    expect(hiddenRule, hidden).toBeDefined();
    expect(declValue(hiddenRule!.body, "white-space")).toBe("nowrap");
    const banner = ".sidebar-layout.sidebar-compact .root-agent-banner";
    const bannerRule = rules.find((x) => x.selectors.includes(banner));
    expect(bannerRule, banner).toBeDefined();
    expect(declValue(bannerRule!.body, "overflow")).toBe("hidden");
    // Padding stays untouched (the row would lose 12px otherwise).
    expect(bannerRule!.body).not.toMatch(/(^|[;\s])padding[\w-]*\s*:/);
  });

  it("4b. accessibility: a group with a real open button and no widget inside a widget", () => {
    sessionsStore.setSessions([rootLive()]);
    const r = renderBanner();
    expect(r.banner.getAttribute("role")).toBe("group");
    expect(r.banner.hasAttribute("tabindex")).toBe(false);
    expect(r.banner.getAttribute("data-ac-testid")).toBe("rootAgent.banner");
    expect(r.banner.getAttribute("data-ac-role")).toBe("button");
    expect(r.banner.getAttribute("data-ac-state")).toBe("live");
    expect(r.banner.getAttribute("aria-disabled")).toBe("false");

    expect(r.open.tagName).toBe("BUTTON");
    expect(r.banner.firstElementChild).toBe(r.open);
    expect(r.open.getAttribute("aria-label")).toBe(r.banner.getAttribute("aria-label"));
    expect(r.open.disabled).toBe(false);

    const widget = "button, a[href], [role='button'], [role='link'], [role='menuitem']";
    for (const button of r.banner.querySelectorAll("button")) {
      expect(button.parentElement!.closest(widget), button.className).toBeNull();
    }
  });

  it("4b. the open button is disabled exactly while the row is busy", async () => {
    sessionsStore.setSessions([rootLive()]);
    const fake = new FakeTransport();
    let release = (): void => undefined;
    fake.onInvoke("switch_session", () => new Promise<void>((done) => (release = done)));
    const r = renderBanner(fake);
    r.open.click();
    expect(r.open.disabled).toBe(true);
    expect(r.banner.getAttribute("aria-disabled")).toBe("true");
    release();
    await waitFor(() => expect(r.open.disabled).toBe(false));
    expect(r.banner.getAttribute("aria-disabled")).toBe("false");
  });

  it("4c. the open button and the bridge's container click() each select exactly once", async () => {
    sessionsStore.setSessions([rootLive()]);
    const r = renderBanner();
    r.open.click();
    await waitFor(() => expect(r.open.disabled).toBe(false));
    expect(selections(r.fake)).toBe(1);
    r.banner.click();
    await waitFor(() => expect(r.open.disabled).toBe(false));
    expect(selections(r.fake)).toBe(2);
  });

  it("4c. mutant kill (dropped guard): toggle and nested-control clicks leave the selection count unchanged", async () => {
    sessionsStore.setSessions([rootLive()]);
    vi.spyOn(voiceRecorder, "autoExecuteSessionId").mockReturnValue("root");
    const cancel = vi.spyOn(voiceRecorder, "cancelAutoExecute").mockImplementation(() => undefined);
    const r = renderBanner();
    r.toggle.click();
    const nested = r.banner.querySelector(".voice-cancel-execute") as HTMLButtonElement;
    expect(nested).not.toBeNull();
    nested.click();
    expect(cancel).toHaveBeenCalledTimes(1);
    await Promise.resolve();
    expect(selections(r.fake)).toBe(0);
  });

  it("4c. mutant kill (narrowed guard): a click on the lifted status dot selects exactly once", async () => {
    sessionsStore.setSessions([rootLive()]);
    const r = renderBanner();
    const dot = r.banner.querySelector(":scope > .session-item-status") as HTMLElement;
    expect(dot).not.toBeNull();
    dot.click();
    await waitFor(() => expect(selections(r.fake)).toBe(1));
  });

  it("4d. every button, onClick or title holder in the banner subtree maps to one of nine declared names", () => {
    const DECLARED = new Set([
      "session-item-mic-cancel",
      "session-item-bridge-icon",
      "session-item-status",
      "voice-cancel-execute",
      "ctx-badge",
      "profile-outdated-badge",
      "root-agent-banner-toggle",
      "root-agent-banner-open",
      "root-agent-banner",
    ]);
    // Explicit named exclusions, never a substring heuristic.
    const STATE_MODIFIERS = new Set(["unavailable"]);
    const ALLOWED_INTERPOLATIONS = new Set(["${dotClass()}"]);
    const read = (file: string) => readFileSync(new URL(`./${file}`, moduleUrl), "utf8");

    // Opening tags, skipping `>` inside braces, quotes and template literals;
    // tags nested in attribute values are scanned as well.
    const tags = (src: string): Array<{ name: string; attrs: string }> => {
      const out: Array<{ name: string; attrs: string }> = [];
      for (let i = 0; i < src.length; i++) {
        if (src[i] !== "<" || !/[A-Za-z]/.test(src[i + 1] ?? "")) continue;
        let j = i + 1;
        let depth = 0;
        let quote: string | null = null;
        for (; j < src.length; j++) {
          const c = src[j];
          if (quote) {
            if (c === quote) quote = null;
          } else if (c === '"' || c === "'" || c === "`") quote = c;
          else if (c === "{") depth++;
          else if (c === "}") depth--;
          else if (c === ">" && depth === 0) break;
        }
        const body = src.slice(i + 1, j);
        const name = /^[\w.]+/.exec(body)![0];
        const attrs = body.slice(name.length);
        out.push({ name, attrs });
        // JSX passed as an attribute (e.g. <Show fallback={<span ...>}>) renders too.
        out.push(...tags(attrs));
        i = j;
      }
      return out;
    };
    const classTokens = (attrs: string): string[] => {
      const m = /\sclass=(?:"([^"]*)"|\{`([^`]*)`\})/.exec(attrs);
      return m ? (m[1] ?? m[2]).split(/\s+/).filter(Boolean) : [];
    };

    const banner = read("RootAgentBanner.tsx");
    const start = banner.lastIndexOf("<div", banner.indexOf('class="root-agent-banner"'));
    const end = banner.indexOf("<Show when={showAgentPicker()}>");
    expect(start).toBeGreaterThan(0);
    expect(end).toBeGreaterThan(start);

    const COMPONENTS: Record<string, string | null> = {
      Show: null,
      ContextBadge: "ContextBadge.tsx",
      ProfileOutdatedBadge: "ProfileOutdatedBadge.tsx",
      TelegramIcon: "TelegramIcon.tsx",
    };
    const sources = [banner.slice(start, end)];
    for (const { name } of tags(sources[0])) {
      if (!/^[A-Z]/.test(name)) continue;
      expect(name in COMPONENTS, `unresolved component <${name}>`).toBe(true);
      const file = COMPONENTS[name];
      if (file) sources.push(read(file));
    }

    const found = new Set<string>();
    for (const src of sources) {
      for (const { name, attrs } of tags(src)) {
        if (/^[A-Z]/.test(name)) continue;
        const hit = name === "button" || /\sonClick=/.test(attrs) || /\stitle=/.test(attrs);
        if (!hit) continue;
        const tokens = classTokens(attrs).filter(
          (t) => !STATE_MODIFIERS.has(t) && !ALLOWED_INTERPOLATIONS.has(t),
        );
        expect(tokens.length, `<${name}${attrs}> has no declared class`).toBeGreaterThan(0);
        for (const t of tokens) found.add(t);
      }
    }
    for (const name of found) expect(DECLARED.has(name), name).toBe(true);
    expect(found.has("root-agent-banner-toggle")).toBe(true);
    expect(found.has("root-agent-banner-open")).toBe(true);
  });

  it("5. compact row composition: DOM kept, no inline style or hiding class, compact + rail side on .sidebar-layout", async () => {
    const fake = new FakeTransport();
    setupApp(fake);
    const rendered = mount(() => <SidebarApp embedded />, fake);
    await appSettled(fake);
    const layout = rendered.root.querySelector(".sidebar-layout") as HTMLElement;
    const banner = rendered.root.querySelector(BANNER_SEL) as HTMLElement;
    const classesOf = () => [...banner.children].map((c) => c.className);
    const expanded = classesOf();
    expect(layout.classList.contains("sidebar-compact")).toBe(false);

    setSidebarCompactMode(true);
    await waitFor(() => expect(layout.classList.contains("sidebar-compact")).toBe(true));
    expect(layout.getAttribute("data-rail-side")).toMatch(/^(left|right)$/);
    expect(banner.closest(".sidebar-layout.sidebar-compact[data-rail-side]")).toBe(layout);
    for (const cls of ["session-item-status", "root-agent-avatar", "root-agent-text", "root-agent-banner-toggle"]) {
      expect(banner.querySelector(`:scope > .${cls}`), cls).not.toBeNull();
    }
    expect(classesOf()).toEqual(expanded);
    expect(banner.getAttribute("style")).toBeNull();
    for (const child of banner.children) expect(child.getAttribute("style"), child.className).toBeNull();
  });

  it("6. the toggle's aria-label and title name currentHotkey(), never a hard-coded shortcut", () => {
    const r = renderBanner();
    expect(r.toggle.getAttribute("aria-label")).toContain(DEFAULT_SIDEBAR_COMPACT_HOTKEY);
    expect(r.toggle.getAttribute("title")).toContain(DEFAULT_SIDEBAR_COMPACT_HOTKEY);
    setSidebarCompactHotkey("Ctrl+Shift+B");
    expect(r.toggle.getAttribute("aria-label")).toContain("Ctrl+Shift+B");
    expect(r.toggle.getAttribute("title")).toContain("Ctrl+Shift+B");
    expect(r.toggle.getAttribute("aria-label")).not.toContain(DEFAULT_SIDEBAR_COMPACT_HOTKEY);
  });

  it("6b. #2519: the toggle is addressable by the UI bridge and reports compact/expanded", () => {
    const r = renderBanner();
    expect(r.root.querySelector('[data-ac-testid="rootAgent.compactToggle"]')).toBe(r.toggle);
    expect(r.toggle.getAttribute("data-ac-role")).toBe("button");
    expect(r.toggle.getAttribute("data-ac-state")).toBe("expanded");
    setSidebarCompactMode(true);
    expect(r.toggle.getAttribute("data-ac-state")).toBe("compact");
  });

  it.each([
    ["right", false, ">>"],
    ["right", true, "<<"],
    ["left", false, "<<"],
    ["left", true, ">>"],
  ] as const)("6c. #2519: %s rail, compact=%s shows %s", (railSide, isCompact, glyph) => {
    const rendered = mount(() => <RootAgentBanner compact={isCompact} railSide={railSide} />, bannerFake());
    const toggle = rendered.root.querySelector(".root-agent-banner-toggle") as HTMLElement;
    expect(toggle.textContent).toBe(glyph);
    // The glyph is the only thing that mirrors.
    expect(toggle.getAttribute("data-ac-state")).toBe(isCompact ? "compact" : "expanded");
    expect(toggle.getAttribute("aria-expanded")).toBe(String(!isCompact));
  });

  it("6d. #2519: SidebarApp hands its rail side to the banner", async () => {
    const fake = new FakeTransport();
    setupApp(fake);
    const rendered = mount(() => <SidebarApp embedded railSide="left" />, fake);
    await appSettled(fake);
    const toggle = rendered.root.querySelector(".root-agent-banner-toggle") as HTMLElement;
    expect(toggle.textContent).toBe(sidebarCompact() ? ">>" : "<<");
  });

  it("7. SidebarApp hydrates the configured shortcut on load and every refresh, without a remount", async () => {
    const fake = new FakeTransport();
    setupApp(fake);
    const rendered = mount(() => <SidebarApp embedded />, fake);
    await appSettled(fake);
    const toggle = rendered.root.querySelector(".root-agent-banner-toggle") as HTMLElement;
    fake.resolve("get_settings", baseSettings({ projectPaths: [], sidebarCompactHotkey: "Ctrl+Shift+B" }));
    await settingsStore.load();
    await waitFor(() => expect(currentHotkey()).toBe("Ctrl+Shift+B"));
    expect(toggle.getAttribute("aria-label")).toContain("Ctrl+Shift+B");

    fake.resolve("get_settings", baseSettings({ projectPaths: [], sidebarCompactHotkey: "Ctrl+Alt+K" }));
    settingsStore.refresh();
    await waitFor(() => expect(currentHotkey()).toBe("Ctrl+Alt+K"));
    expect(rendered.root.querySelector(".root-agent-banner-toggle")).toBe(toggle);

    const absent = baseSettings({ projectPaths: [] });
    delete absent.sidebarCompactHotkey;
    fake.resolve("get_settings", absent);
    const errors = vi.spyOn(console, "error");
    settingsStore.refresh();
    await waitFor(() => expect(settingsStore.current).toBe(absent));
    await waitFor(() => expect(currentHotkey()).toBe(DEFAULT_SIDEBAR_COMPACT_HOTKEY));
    expect(toggle.getAttribute("aria-label")).toContain(DEFAULT_SIDEBAR_COMPACT_HOTKEY);
    expect(errors).not.toHaveBeenCalled();
  });

  it("8. negative control for 7: the banner alone never hydrates the shortcut", async () => {
    const fake = bannerFake();
    const r = renderBanner(fake);
    fake.resolve("get_settings", baseSettings({ projectPaths: [], sidebarCompactHotkey: "Ctrl+Shift+B" }));
    await settingsStore.load();
    expect(settingsStore.current?.sidebarCompactHotkey).toBe("Ctrl+Shift+B");
    settingsStore.refresh();
    await Promise.resolve();
    await Promise.resolve();
    expect(currentHotkey()).toBe(DEFAULT_SIDEBAR_COMPACT_HOTKEY);
    expect(r.toggle.getAttribute("aria-label")).toContain(DEFAULT_SIDEBAR_COMPACT_HOTKEY);
  });
});
