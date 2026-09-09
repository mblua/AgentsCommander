// @vitest-environment jsdom
// #1871 section 10.3 - the one test this plan will not ship without. The root's
// absent menu items must be absent because their DATA is empty, not because the
// root is named in an exclusion list, so that the day the root carries repo
// data the entries appear with no code change.
//
// The defeat this file is shaped against: two separately mounted cases (a root
// with repos, a root without) are passed by
//   `root.gitRepos.length ? reposSpec(HARDCODED_REPOS, ...) : false`
// which is exactly the length-gated constant section 2 forbids. So everything
// in the first test happens on ONE mount: four payload transitions across two
// opens, with count, text, order, title and click payload all asserted against
// the payload that is actually on screen.
//
// No fake timers in this file. The surface registers its window listeners in a
// zero-delay setTimeout, so every open is followed by one macrotask hop before
// any window-level dismissal; the DOM reads themselves are synchronous.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import RootAgentBanner from "../components/RootAgentBanner";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  bridge,
  contextMenu,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import { bridgesStore } from "../stores/bridges";
import type { SessionRepo, TelegramBotConfig } from "../../shared/types";

const payloadA: SessionRepo[] = [
  { label: "AgentsCommander", sourcePath: "D:\\0_repos\\AgentsCommander_iac\\repo-AgentsCommander", branch: null, dirty: null },
  { label: "personal", sourcePath: "D:\\0_repos\\AgentsCommander_iac\\repo-personal", branch: null, dirty: null },
];

// A different non-empty payload: three repos, none of whose labels or
// sourcePaths appear in payload A, in a deliberately unsorted order.
const payloadB: SessionRepo[] = [
  { label: "zeta", sourcePath: "D:\\elsewhere\\repo-zeta", branch: null, dirty: null },
  { label: "alpha", sourcePath: "D:\\elsewhere\\repo-alpha", branch: null, dirty: null },
  { label: "mid", sourcePath: "D:\\elsewhere\\repo-mid", branch: null, dirty: null },
];

const BOT_1: TelegramBotConfig = { id: "b1", label: "Ops bot", token: "t1", chatId: 1, color: "#ff0000" };
const BOT_2: TelegramBotConfig = { id: "b2", label: "Dev bot", token: "t2", chatId: 2, color: "#00ff00" };

const q = (testid: string): HTMLElement | null =>
  document.querySelector<HTMLElement>(`[data-ac-testid="${testid}"]`);
const banner = (): HTMLElement => {
  const el = q("rootAgent.banner");
  if (!el) throw new Error("rootAgent.banner did not render");
  return el;
};
const menuItems = (): string[] =>
  Array.from(q("rootAgent.menu")!.querySelectorAll('[data-ac-role="menuitem"]')).map(
    (el) => el.getAttribute("data-ac-testid") ?? "",
  );
const hop = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));
const click = (el: Element): boolean =>
  el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
/** A left click that lands on something which stops nothing. */
const clickElsewhere = (): boolean => click(document.body);

/** Open the root menu and prove it opened, through an always-on item. */
async function openRootMenu(): Promise<void> {
  contextMenu(banner());
  await hop();
  expect(q("rootAgent.restart")).not.toBeNull();
}

function liveRoot(overrides: Partial<Parameters<typeof session>[0]> = {}) {
  return session({
    id: "root-1",
    name: "Agent's Commander",
    isRootAgent: true,
    status: "running",
    ...overrides,
  });
}

function newFake(): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("open_in_explorer", undefined);
  fake.resolve("get_settings", baseSettings());
  fake.resolve("telegram_attach", undefined);
  fake.resolve("telegram_detach", undefined);
  fake.resolve("destroy_session", undefined);
  return fake;
}

let cleanupDom: (() => void) | null = null;
let cleanupRender: (() => void) | null = null;

function renderRoot(fake: FakeTransport): FakeTransport {
  const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
  cleanupRender = rendered.cleanup;
  return rendered.fake;
}

describe("#1871 root menu is derived from data", () => {
  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupRender?.();
    cleanupRender = null;
    resetUiStoresForTests();
    cleanupDom?.();
    cleanupDom = null;
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("renders the root's repo entries from the store, following four payloads across two opens on one mount", async () => {
    // 1. A live root carrying two repos.
    const root = liveRoot({ gitRepos: payloadA });
    sessionsStore.setSessions([root]);
    // 2. The fixture first. If this line is ever softened the rest of the test
    //    is meaningless: an empty fixture makes the entries absent whether the
    //    record is derived or hardcoded.
    expect(sessionsStore.sessions[0].gitRepos).toHaveLength(2);

    // 3, 4.
    const fake = renderRoot(newFake());
    await openRootMenu();

    // 5. The same-mount transition. No unmount, no re-render by hand, no reopen.
    sessionsStore.setGitRepos("root-1", payloadB);
    expect(q("rootAgent.menu.repo.0")).not.toBeNull();
    expect(q("rootAgent.menu.repo.1")).not.toBeNull();
    expect(q("rootAgent.menu.repo.2")).not.toBeNull();
    expect(q("rootAgent.menu.repo.3")).toBeNull();
    const entries = [0, 1, 2].map((i) => q(`rootAgent.menu.repo.${i}`)!);
    expect(entries.map((el) => el.textContent?.trim())).toEqual(["zeta", "alpha", "mid"]);
    expect(entries.map((el) => el.getAttribute("title"))).toEqual(payloadB.map((r) => r.sourcePath));

    // 6. Position within the catalogue, against the three-repo state on screen.
    const head = ["rootAgent.restart", "rootAgent.codingAgent", "rootAgent.openFolder"];
    const repos = payloadB.map((_, i) => `rootAgent.menu.repo.${i}`);
    const tail = ["rootAgent.close", "rootAgent.menu.detachToggle", "rootAgent.menu.telegram"];
    expect(menuItems()).toEqual([
      "rootAgent.restart",
      "rootAgent.codingAgent",
      "rootAgent.openFolder",
      "rootAgent.menu.repo.0",
      "rootAgent.menu.repo.1",
      "rootAgent.menu.repo.2",
      "rootAgent.close",
      "rootAgent.menu.detachToggle",
      "rootAgent.menu.telegram",
    ]);
    expect(menuItems()).toEqual([...head, ...repos, ...tail]);

    // 7. Click through the exact sourcePath, and watch the menu close.
    click(q("rootAgent.menu.repo.1")!);
    expect(fake.lastCall("open_in_explorer")?.args).toEqual({ path: payloadB[1].sourcePath });
    expect(q("rootAgent.menu")).toBeNull();

    // 8. Reopen, then two more payloads.
    await openRootMenu();
    sessionsStore.setGitRepos("root-1", []);
    expect(q("rootAgent.menu.repo.0")).toBeNull();
    expect(q("rootAgent.restart")).not.toBeNull();
    sessionsStore.setGitRepos("root-1", [payloadA[1]]);
    const single = q("rootAgent.menu.repo.0");
    expect(single).not.toBeNull();
    expect(single!.textContent?.trim()).toBe("personal");
    expect(single!.getAttribute("title")).toBe(payloadA[1].sourcePath);
    expect(q("rootAgent.menu.repo.1")).toBeNull();
  });

  describe("telegram epoch and its guards", () => {
    it("zero bots: selecting the toggle closes the menu and attaches nothing", async () => {
      sessionsStore.setSessions([liveRoot()]);
      const fake = newFake();
      fake.resolve("get_settings", baseSettings({ telegramBots: [] }));
      renderRoot(fake);
      await openRootMenu();
      click(q("rootAgent.menu.telegram")!);
      await waitFor(() => expect(q("rootAgent.menu")).toBeNull());
      expect(fake.callsFor("telegram_attach")).toHaveLength(0);
    });

    it("one bot: selecting the toggle closes the menu, then attaches that bot", async () => {
      sessionsStore.setSessions([liveRoot()]);
      const fake = newFake();
      fake.resolve("get_settings", baseSettings({ telegramBots: [BOT_1] }));
      renderRoot(fake);
      await openRootMenu();
      click(q("rootAgent.menu.telegram")!);
      await waitFor(() => expect(fake.lastCall("telegram_attach")?.args).toEqual({ sessionId: "root-1", botId: "b1" }));
      expect(q("rootAgent.menu")).toBeNull();
    });

    it("many bots: the menu stays open with the rows in order; picking one closes it and attaches that bot", async () => {
      sessionsStore.setSessions([liveRoot()]);
      const fake = newFake();
      fake.resolve("get_settings", baseSettings({ telegramBots: [BOT_1, BOT_2] }));
      renderRoot(fake);
      await openRootMenu();
      click(q("rootAgent.menu.telegram")!);
      await waitFor(() => expect(q("rootAgent.menu.telegram.bot.b1")).not.toBeNull());
      expect(q("rootAgent.menu")).not.toBeNull();
      const rows = Array.from(q("rootAgent.menu")!.querySelectorAll('[data-ac-testid^="rootAgent.menu.telegram.bot."]'));
      expect(rows.map((el) => el.getAttribute("data-ac-testid"))).toEqual([
        "rootAgent.menu.telegram.bot.b1",
        "rootAgent.menu.telegram.bot.b2",
      ]);
      expect(rows.map((el) => el.textContent?.trim())).toEqual(["Ops bot", "Dev bot"]);
      click(q("rootAgent.menu.telegram.bot.b2")!);
      expect(q("rootAgent.menu")).toBeNull();
      await waitFor(() => expect(fake.lastCall("telegram_attach")?.args).toEqual({ sessionId: "root-1", botId: "b2" }));
    });

    it("stale await, dismissed: a settings fetch that resolves after a dismiss publishes nothing", async () => {
      sessionsStore.setSessions([liveRoot()]);
      const fake = newFake();
      let resolveSettings: ((value: unknown) => void) | null = null;
      fake.onInvoke("get_settings", () => new Promise((resolve) => { resolveSettings = resolve; }));
      renderRoot(fake);
      await openRootMenu();
      click(q("rootAgent.menu.telegram")!);
      await hop();
      expect(resolveSettings).not.toBeNull();
      clickElsewhere();
      expect(q("rootAgent.menu")).toBeNull();
      resolveSettings!(baseSettings({ telegramBots: [BOT_1, BOT_2] }));
      await hop();
      await hop();
      expect(q("rootAgent.menu.telegram.bot.b1")).toBeNull();
      expect(q("rootAgent.menu")).toBeNull();
    });

    it("stale await, reopened: a settings fetch from the previous open never publishes into the new menu", async () => {
      sessionsStore.setSessions([liveRoot()]);
      const fake = newFake();
      let resolveSettings: ((value: unknown) => void) | null = null;
      fake.onInvoke("get_settings", () => new Promise((resolve) => { resolveSettings = resolve; }));
      renderRoot(fake);
      await openRootMenu();
      click(q("rootAgent.menu.telegram")!);
      await hop();
      expect(resolveSettings).not.toBeNull();
      // Right-click the banner again WITHOUT dismissing: handleContextMenu stops
      // propagation, so the surface's window contextmenu listener never sees it.
      contextMenu(banner());
      await hop();
      expect(q("rootAgent.menu")).not.toBeNull();
      expect(q("rootAgent.restart")).not.toBeNull();
      resolveSettings!(baseSettings({ telegramBots: [BOT_1, BOT_2] }));
      await hop();
      await hop();
      expect(q("rootAgent.menu")).not.toBeNull();
      expect(q("rootAgent.menu.telegram.bot.b1")).toBeNull();
      expect(q("rootAgent.menu.telegram.bot.b2")).toBeNull();
    });

    it("session replaced under an expanded list: a bot click attaches nothing", async () => {
      sessionsStore.setSessions([liveRoot()]);
      const fake = newFake();
      fake.resolve("get_settings", baseSettings({ telegramBots: [BOT_1, BOT_2] }));
      renderRoot(fake);
      await openRootMenu();
      click(q("rootAgent.menu.telegram")!);
      await waitFor(() => expect(q("rootAgent.menu.telegram.bot.b1")).not.toBeNull());
      sessionsStore.setSessions([liveRoot({ id: "root-2" })]);
      expect(q("rootAgent.menu.telegram.bot.b1")).not.toBeNull();
      click(q("rootAgent.menu.telegram.bot.b1")!);
      await hop();
      expect(fake.callsFor("telegram_attach")).toHaveLength(0);
    });

    describe("rejections: every command a menu path awaits is caught", () => {
      type Row = {
        name: string;
        cmd: string;
        setup: (fake: FakeTransport) => void;
        act: () => Promise<void>;
        menuOpenAfter: boolean;
      };
      const rows: Row[] = [
        {
          name: "get_settings via the unbridged toggle",
          cmd: "get_settings",
          setup: () => {},
          act: async () => {
            click(q("rootAgent.menu.telegram")!);
          },
          menuOpenAfter: true,
        },
        {
          name: "telegram_detach via the bridged toggle",
          cmd: "telegram_detach",
          setup: () => {
            bridgesStore.setBridges([bridge({ sessionId: "root-1" })]);
          },
          act: async () => {
            click(q("rootAgent.menu.telegram")!);
          },
          menuOpenAfter: false,
        },
        {
          name: "telegram_attach via the one-bot toggle",
          cmd: "telegram_attach",
          setup: (fake) => {
            fake.resolve("get_settings", baseSettings({ telegramBots: [BOT_1] }));
          },
          act: async () => {
            click(q("rootAgent.menu.telegram")!);
          },
          menuOpenAfter: false,
        },
        {
          name: "telegram_attach via a bot row",
          cmd: "telegram_attach",
          setup: (fake) => {
            fake.resolve("get_settings", baseSettings({ telegramBots: [BOT_1, BOT_2] }));
          },
          act: async () => {
            click(q("rootAgent.menu.telegram")!);
            await waitFor(() => expect(q("rootAgent.menu.telegram.bot.b1")).not.toBeNull());
            click(q("rootAgent.menu.telegram.bot.b1")!);
          },
          menuOpenAfter: false,
        },
        {
          name: "open_in_explorer via rootAgent.openFolder (menuOpenFolder)",
          cmd: "open_in_explorer",
          setup: () => {},
          act: async () => {
            click(q("rootAgent.openFolder")!);
          },
          menuOpenAfter: false,
        },
        {
          name: "open_in_explorer via a repo entry (menuOpenRepo)",
          cmd: "open_in_explorer",
          setup: () => {
            sessionsStore.setGitRepos("root-1", payloadA);
          },
          act: async () => {
            click(q("rootAgent.menu.repo.0")!);
          },
          menuOpenAfter: false,
        },
        {
          name: "destroy_session via rootAgent.close (menuClose)",
          cmd: "destroy_session",
          setup: () => {},
          act: async () => {
            click(q("rootAgent.close")!);
          },
          menuOpenAfter: false,
        },
      ];

      it.each(rows)("$name rejects: no unhandled rejection, console.error exactly once", async (row) => {
        sessionsStore.setSessions([liveRoot()]);
        const fake = newFake();
        row.setup(fake);
        fake.reject(row.cmd, "boom");
        const unhandled = vi.fn();
        window.addEventListener("unhandledrejection", unhandled);
        const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
        try {
          renderRoot(fake);
          await openRootMenu();
          await row.act();
          await waitFor(() => expect(errorSpy).toHaveBeenCalledTimes(1));
          await hop();
          expect(errorSpy).toHaveBeenCalledTimes(1);
          expect(unhandled).not.toHaveBeenCalled();
          if (row.menuOpenAfter) expect(q("rootAgent.menu")).not.toBeNull();
          else expect(q("rootAgent.menu")).toBeNull();
        } finally {
          window.removeEventListener("unhandledrejection", unhandled);
        }
      });
    });

    it("reclamp: an expanded bot list that would overflow moves the menu up to innerHeight - height - 8", async () => {
      let menuHeight = 100;
      vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
        const height = this.classList.contains("session-context-menu") ? menuHeight : 0;
        const width = this.classList.contains("session-context-menu") ? 200 : 0;
        return { left: 0, top: 0, width, height, right: width, bottom: height, x: 0, y: 0, toJSON: () => ({}) } as DOMRect;
      });
      const previousHeight = window.innerHeight;
      Object.defineProperty(window, "innerHeight", { configurable: true, writable: true, value: 400 });
      try {
        sessionsStore.setSessions([liveRoot()]);
        const fake = newFake();
        fake.resolve("get_settings", baseSettings({ telegramBots: [BOT_1, BOT_2] }));
        renderRoot(fake);
        await openRootMenu();
        // The harness contextMenu() requests y = 96; with height 100 it fits.
        expect(q("rootAgent.menu")!.style.top).toBe("96px");
        menuHeight = 300;
        click(q("rootAgent.menu.telegram")!);
        await waitFor(() => expect(q("rootAgent.menu.telegram.bot.b1")).not.toBeNull());
        await waitFor(() => expect(q("rootAgent.menu")!.style.top).toBe("92px"));
      } finally {
        Object.defineProperty(window, "innerHeight", { configurable: true, writable: true, value: previousHeight });
      }
    });
  });

  describe("#1896: the menu's bot list dismisses with the menu", () => {
    it("a left click on something that stops nothing dismisses the menu and clears its bot list", async () => {
      sessionsStore.setSessions([liveRoot()]);
      const fake = newFake();
      fake.resolve("get_settings", baseSettings({ telegramBots: [BOT_1, BOT_2] }));
      renderRoot(fake);
      await openRootMenu();
      click(q("rootAgent.menu.telegram")!);
      await waitFor(() => expect(q("rootAgent.menu.telegram.bot.b1")).not.toBeNull());
      clickElsewhere();
      expect(q("rootAgent.menu")).toBeNull();
      expect(q("rootAgent.menu.telegram.bot.b1")).toBeNull();
    });
  });

  describe("the root's exact key census", () => {
    // Raw source, immune to import elision, exactly as the import watchdogs
    // read it. An exact allowlist, not a predicate: an omitted key, an alias, a
    // cast, a spread or a two-line split all fail here, and none of them can
    // be caught by a regex over one line.
    const RAW = import.meta.glob<string>("../components/RootAgentBanner.tsx", {
      query: "?raw",
      import: "default",
      eager: true,
    });

    const EXPECTED_KEYS = [
      "addToGroup",
      "clearTaskTitle",
      "close",
      "codingAgent",
      "deleteAgent",
      "detach",
      "editTaskTitle",
      "matrixFolder",
      "openFolder",
      "repos",
      "restart",
      "telegram",
    ];

    const EXPECTED_VALUES: Record<string, string> = {
      restart: "{ onSelect: () => void handleRestart(), disabled: !rootSession() }",
      codingAgent: "{ onSelect: handleCodingAgent }",
      openFolder: "openFolderSpec(rootSession(), { onSelect: () => void menuOpenFolder() })",
      repos:
        "reposSpec(rootSession()?.gitRepos ?? [], { browseItems: () => [], onOpenRepo: (p) => void menuOpenRepo(p), onOpenBrowse: () => {}, })",
      matrixFolder: "matrixFolderSpec(undefined)",
      close: "closeSpec(rootSession(), { onSelect: () => void menuClose() })",
      deleteAgent: "deleteAgentSpec(undefined)",
      detach: "detachSpec(hasLivePty(), isDetached(), () => void handleContextDetachToggle())",
      telegram:
        "telegramSpec(hasLivePty() ? rootSession() : undefined, { on: !!bridge(), bridgeColor: bridge()?.color ?? null, bots: menuTelegramBots()?.bots ?? null, onSelect: () => void menuTelegram(), onSelectBot: (id) => void menuSelectBot(id), })",
      addToGroup: "addToGroupSpec(undefined)",
      editTaskTitle: "taskTitleSpec(undefined)",
      clearTaskTitle: "clearTaskTitleSpec(undefined)",
    };

    function capsLiteral(source: string): string {
      const marker = "caps={{";
      const start = source.indexOf(marker);
      if (start === -1) throw new Error("caps={{ not found in RootAgentBanner.tsx");
      const open = start + marker.length - 1; // the inner '{'
      let depth = 0;
      for (let i = open; i < source.length; i += 1) {
        const ch = source[i];
        if (ch === "{") depth += 1;
        else if (ch === "}") {
          depth -= 1;
          if (depth === 0) return source.slice(open + 1, i);
        }
      }
      throw new Error("unbalanced caps literal");
    }

    function topLevelEntries(body: string): string[] {
      const entries: string[] = [];
      let depth = 0;
      let quote: string | null = null;
      let current = "";
      for (const ch of body) {
        if (quote) {
          current += ch;
          if (ch === quote) quote = null;
          continue;
        }
        if (ch === '"' || ch === "'" || ch === "`") {
          quote = ch;
          current += ch;
          continue;
        }
        if (ch === "{" || ch === "[" || ch === "(") depth += 1;
        if (ch === "}" || ch === "]" || ch === ")") depth -= 1;
        if (ch === "," && depth === 0) {
          entries.push(current);
          current = "";
          continue;
        }
        current += ch;
      }
      entries.push(current);
      return entries.map((e) => e.replace(/\s+/g, " ").trim()).filter((e) => e !== "");
    }

    function parseRecord(): Record<string, string> {
      const sources = Object.values(RAW);
      expect(sources).toHaveLength(1);
      const record: Record<string, string> = {};
      for (const entry of topLevelEntries(capsLiteral(sources[0]))) {
        const colon = entry.indexOf(":");
        expect(colon).toBeGreaterThan(0);
        const key = entry.slice(0, colon).trim();
        const value = entry.slice(colon + 1).trim();
        expect(record[key]).toBeUndefined();
        record[key] = value;
      }
      return record;
    }

    it("has exactly the twelve catalogue keys", () => {
      expect(Object.keys(parseRecord()).sort()).toEqual(EXPECTED_KEYS);
    });

    it("has exactly the twelve helper-derived right-hand sides", () => {
      expect(parseRecord()).toEqual(EXPECTED_VALUES);
    });
  });
});
