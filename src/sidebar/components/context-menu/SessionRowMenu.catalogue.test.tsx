// @vitest-environment jsdom
// #1871 section 10.2 - the catalogue, black-box, driven by synthetic
// SessionRowMenuCaps records. EXPECTED below is written out as literals and is
// never imported from the component: a test that mapped over the component's
// own catalogue constant would pass for any order, any label and any icon.
// No fake timers in this file: the harness waitFor is never used, and every
// assertion reads the DOM synchronously after the render.
import { batch, createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SessionRowMenu from "./SessionRowMenu";
import type {
  AddToGroupSpec,
  EditTaskTitleSpec,
  GroupChoice,
  ReposSpec,
  SessionRowMenuCaps,
  SessionRowMenuItemId,
  TelegramSpec,
} from "./session-row-menu-types";
import type { SessionRepo, TelegramBotConfig } from "../../../shared/types";
import {
  installBrowserDomStubs,
  installDeterministicAnimationFrames,
} from "../../../shared/testing/ui-harness";

type IconExpectation =
  | { kind: "emoji"; text: string }
  | { kind: "svg"; selector: string; d?: string };

interface ExpectedRow {
  id: SessionRowMenuItemId;
  group: 1 | 2 | 3 | 4 | 5;
  testid: string;
  label: string;
  icon: IconExpectation;
  danger: boolean;
  dimWhenDisabled: boolean;
  class: string;
  /** Toggles only: the `on: false` state. */
  labelOff?: string;
  iconOff?: IconExpectation;
}

const DETACH_D = "ZM18.2197 8.46967";
const REATTACH_D = "ZM12.5303 8.46967";
const TELEGRAM_D = "M2.01 21L23 12";
const TRASH_D = "M16.5 4.478";

// Written out in the test file. NOT imported from SessionRowMenu.tsx.
const EXPECTED: ReadonlyArray<ExpectedRow> = [
  { id: "restart", group: 1, testid: "rootAgent.restart", label: "Restart Session", icon: { kind: "emoji", text: "\u21BA" }, danger: true, dimWhenDisabled: false, class: "session-context-option context-option-danger" },
  { id: "codingAgent", group: 1, testid: "rootAgent.codingAgent", label: "Coding Agent", icon: { kind: "emoji", text: "\u{1F916}" }, danger: false, dimWhenDisabled: false, class: "session-context-option" },
  { id: "openFolder", group: 1, testid: "rootAgent.openFolder", label: "Open Folder", icon: { kind: "emoji", text: "\u{1F4C2}" }, danger: false, dimWhenDisabled: false, class: "session-context-option" },
  { id: "repos", group: 1, testid: "rootAgent.menu.repo.0", label: "AgentsCommander", icon: { kind: "svg", selector: "svg.session-context-repo-icon" }, danger: false, dimWhenDisabled: false, class: "session-context-option session-context-repo-option" },
  { id: "matrixFolder", group: 1, testid: "rootAgent.menu.matrixFolder", label: "Open Matrix folder", icon: { kind: "svg", selector: "svg.session-context-matrix-icon" }, danger: false, dimWhenDisabled: false, class: "session-context-option" },
  { id: "close", group: 1, testid: "rootAgent.close", label: "Close Session (Ctrl+Shift+W)", icon: { kind: "emoji", text: "\u2715" }, danger: true, dimWhenDisabled: false, class: "session-context-option context-option-danger" },
  { id: "deleteAgent", group: 2, testid: "agent.action.delete.dev-webpage-ui", label: "Delete", icon: { kind: "svg", selector: "svg", d: TRASH_D }, danger: true, dimWhenDisabled: false, class: "session-context-option context-option-danger" },
  { id: "detach", group: 3, testid: "rootAgent.menu.detachToggle", label: "Re-attach session", labelOff: "Detach session", icon: { kind: "svg", selector: "svg.session-context-detach-icon", d: REATTACH_D }, iconOff: { kind: "svg", selector: "svg.session-context-detach-icon", d: DETACH_D }, danger: false, dimWhenDisabled: false, class: "session-context-option" },
  { id: "telegram", group: 3, testid: "rootAgent.menu.telegram", label: "Detach Telegram", labelOff: "Attach Telegram", icon: { kind: "svg", selector: "svg", d: TELEGRAM_D }, iconOff: { kind: "svg", selector: "svg", d: TELEGRAM_D }, danger: false, dimWhenDisabled: false, class: "session-context-option" },
  { id: "addToGroup", group: 4, testid: "replica.wg-1-dev-team.groups.trigger", label: "Add to Group", icon: { kind: "svg", selector: "svg.session-context-group-add-icon" }, danger: false, dimWhenDisabled: false, class: "session-context-option session-context-submenu-trigger" },
  { id: "editTaskTitle", group: 5, testid: "rootAgent.menu.editTaskTitle", label: "Edit TASK title", icon: { kind: "emoji", text: "\u270E" }, danger: false, dimWhenDisabled: false, class: "session-context-option" },
  { id: "clearTaskTitle", group: 5, testid: "rootAgent.menu.clearTaskTitle", label: "Clear task title", icon: { kind: "emoji", text: "\u{1F9F9}" }, danger: false, dimWhenDisabled: true, class: "session-context-option" },
];

const ALL_IDS: SessionRowMenuItemId[] = EXPECTED.map((row) => row.id);

const noop = (): void => {};

const REPO_A: SessionRepo = { label: "AgentsCommander", sourcePath: "D:\\repos\\AgentsCommander", branch: null, dirty: null };
const REPO_B: SessionRepo = { label: "personal", sourcePath: "D:\\repos\\personal", branch: null, dirty: null };
const BOT_1: TelegramBotConfig = { id: "b1", label: "Ops bot", token: "t1", chatId: 1, color: "#ff0000" };
const BOT_2: TelegramBotConfig = { id: "b2", label: "Dev bot", token: "t2", chatId: 2, color: "#00ff00" };

const GROUP_TEST_IDS = {
  trigger: "replica.wg-1-dev-team.groups.trigger",
  flyout: "replica.wg-1-dev-team.groups.flyout",
  nonstop: "replica.wg-1-dev-team.groups.nonstop",
  choice: (id: string) => `replica.wg-1-dev-team.groups.${id}`,
  create: "replica.wg-1-dev-team.groups.create",
  createInput: "replica.groups.create.input",
  createSave: "replica.groups.create.save",
  menuError: "replica.groups.error",
};

function groupSpec(overrides: Partial<AddToGroupSpec> = {}): AddToGroupSpec {
  return {
    choices: [
      { id: "nonstop", name: "Non-stop", checked: false, disabled: false, title: "Watch this room in the Non-stop group" },
      { id: "g1", name: "Group 1", checked: true, disabled: false, title: "Remove this room from the group" },
    ],
    onToggle: noop,
    emptyNote: null,
    storeError: null,
    menuError: null,
    create: { active: false, draft: "", onDraft: noop, onStart: noop, onSave: noop },
    testIds: GROUP_TEST_IDS,
    ...overrides,
  };
}

function titleSpec(overrides: Partial<EditTaskTitleSpec> = {}): EditTaskTitleSpec {
  return {
    editing: false,
    draft: "",
    busy: false,
    error: null,
    onDraft: noop,
    onStart: noop,
    onSave: noop,
    onCancel: noop,
    ...overrides,
  };
}

function reposSpecOf(repos: SessionRepo[], overrides: Partial<ReposSpec> = {}): ReposSpec {
  return { repos, browseItems: () => [], onOpenRepo: noop, onOpenBrowse: noop, ...overrides };
}

function telegramSpecOf(on: boolean, overrides: Partial<TelegramSpec> = {}): TelegramSpec {
  return { on, onSelect: noop, bridgeColor: on ? "#4ade80" : null, bots: null, onSelectBot: noop, ...overrides };
}

/** The synthetic "on" record for one item id. Toggles take their state. */
function specFor(id: SessionRowMenuItemId, on = true): SessionRowMenuCaps {
  switch (id) {
    case "restart":
      return { restart: { onSelect: noop } };
    case "codingAgent":
      return { codingAgent: { onSelect: noop } };
    case "openFolder":
      return { openFolder: { onSelect: noop } };
    case "repos":
      return { repos: reposSpecOf([REPO_A]) };
    case "matrixFolder":
      return { matrixFolder: { onSelect: noop } };
    case "close":
      return { close: { onSelect: noop } };
    case "deleteAgent":
      return { deleteAgent: { onSelect: noop, testId: "agent.action.delete.dev-webpage-ui" } };
    case "detach":
      return { detach: { on, onSelect: noop } };
    case "telegram":
      return { telegram: telegramSpecOf(on) };
    case "addToGroup":
      return { addToGroup: groupSpec() };
    case "editTaskTitle":
      return { editTaskTitle: titleSpec() };
    case "clearTaskTitle":
      return { clearTaskTitle: { onSelect: noop } };
  }
}

function allOn(): SessionRowMenuCaps {
  let caps: SessionRowMenuCaps = {};
  for (const id of ALL_IDS) caps = { ...caps, ...specFor(id) };
  return caps;
}

function groupsOn(groups: number[]): SessionRowMenuCaps {
  let caps: SessionRowMenuCaps = {};
  for (const row of EXPECTED) {
    if (groups.includes(row.group)) caps = { ...caps, ...specFor(row.id) };
  }
  return caps;
}

const q = (testid: string): HTMLElement | null =>
  document.querySelector<HTMLElement>(`[data-ac-testid="${testid}"]`);
const container = (prefix = "rootAgent"): HTMLElement => {
  const el = q(`${prefix}.menu`);
  if (!el) throw new Error(`menu container ${prefix}.menu did not render`);
  return el;
};

/** The label text: every child node except the icon span and the submenu arrow. */
function labelOf(el: Element): string {
  return Array.from(el.childNodes)
    .filter(
      (node) =>
        !(
          node instanceof Element &&
          (node.classList.contains("session-context-option-icon") ||
            node.classList.contains("session-context-submenu-arrow"))
        ),
    )
    .map((node) => node.textContent ?? "")
    .join("")
    .trim();
}

function iconOf(el: Element): Element {
  const icon = el.querySelector(".session-context-option-icon");
  if (!icon) throw new Error("no icon span");
  return icon;
}

function assertIcon(el: Element, expected: IconExpectation): void {
  const icon = iconOf(el);
  if (expected.kind === "emoji") {
    expect(icon.textContent).toBe(expected.text);
    expect(icon.querySelector("svg")).toBeNull();
  } else {
    const svg = icon.querySelector(expected.selector);
    expect(svg).not.toBeNull();
    if (expected.d) {
      const d = svg!.querySelector("path")?.getAttribute("d") ?? "";
      expect(d).toContain(expected.d);
    }
  }
}

const mouseEnter = (el: Element): boolean =>
  el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false, cancelable: true }));
const click = (el: Element): boolean =>
  el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
const keyDown = (el: Element, key: string): boolean =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
const typeInto = (el: HTMLInputElement, value: string): void => {
  el.value = value;
  el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: value }));
};
const hop = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

interface Mounted {
  setCaps: (caps: SessionRowMenuCaps) => void;
  dispose: () => void;
}

let mounted: Mounted[] = [];
let cleanupDom: (() => void) | null = null;

function mount(
  caps: SessionRowMenuCaps,
  opts: { prefix?: string; onDismiss?: () => void } = {},
): Mounted {
  const root = document.createElement("div");
  document.body.appendChild(root);
  const [capsSignal, setCapsSignal] = createSignal<SessionRowMenuCaps>(caps);
  const dispose = render(
    () => (
      <SessionRowMenu
        open={true}
        x={10}
        y={10}
        testIdPrefix={opts.prefix ?? "rootAgent"}
        caps={capsSignal()}
        onDismiss={opts.onDismiss ?? noop}
      />
    ),
    root,
  );
  const handle: Mounted = {
    setCaps: (next) => batch(() => setCapsSignal(next)),
    dispose: () => {
      dispose();
      root.remove();
    },
  };
  mounted.push(handle);
  return handle;
}

describe("#1871 SessionRowMenu catalogue", () => {
  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
  });

  afterEach(() => {
    for (const handle of mounted) handle.dispose();
    mounted = [];
    cleanupDom?.();
    cleanupDom = null;
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  // 1. One render test per shape.
  describe("one render test per shape", () => {
    it("action: restart renders one button with class, label, icon and role", () => {
      mount(specFor("restart"));
      const button = q("rootAgent.restart")!;
      expect(button).not.toBeNull();
      expect(button.tagName).toBe("BUTTON");
      expect(button.getAttribute("class")).toBe("session-context-option context-option-danger");
      expect(labelOf(button)).toBe("Restart Session");
      assertIcon(button, { kind: "emoji", text: "\u21BA" });
      expect(button.getAttribute("data-ac-role")).toBe("menuitem");
    });

    it("toggle: detach renders one button with two labels, two icons and data-ac-state", () => {
      const handle = mount(specFor("detach", false));
      const button = q("rootAgent.menu.detachToggle")!;
      expect(button.getAttribute("class")).toBe("session-context-option");
      expect(labelOf(button)).toBe("Detach session");
      assertIcon(button, { kind: "svg", selector: "svg.session-context-detach-icon", d: DETACH_D });
      expect(button.getAttribute("data-ac-role")).toBe("menuitem");
      expect(button.getAttribute("data-ac-state")).toBe("attached");
      handle.setCaps(specFor("detach", true));
      const flipped = q("rootAgent.menu.detachToggle")!;
      expect(labelOf(flipped)).toBe("Re-attach session");
      assertIcon(flipped, { kind: "svg", selector: "svg.session-context-detach-icon", d: REATTACH_D });
      expect(flipped.getAttribute("data-ac-state")).toBe("detached");
    });

    it("flyout: a repo entry renders a trigger, its arrow, and a flyout panel on hover", () => {
      mount({
        repos: reposSpecOf([REPO_A], {
          browseItems: () => [{ id: "main", label: "Open main", url: "https://example.test/main" }],
        }),
      });
      const trigger = q("rootAgent.menu.repo.0")!;
      expect(trigger).not.toBeNull();
      expect(trigger.getAttribute("class")).toBe("session-context-option session-context-repo-option");
      expect(labelOf(trigger)).toBe("AgentsCommander");
      assertIcon(trigger, { kind: "svg", selector: "svg.session-context-repo-icon" });
      expect(trigger.getAttribute("data-ac-role")).toBe("menuitem");
      expect(q("rootAgent.menu.repo.0.browse.arrow")).not.toBeNull();
      expect(document.querySelector(".session-context-flyout")).toBeNull();
      mouseEnter(trigger);
      const flyout = document.querySelector(".session-context-flyout");
      expect(flyout).not.toBeNull();
      expect(flyout!.getAttribute("data-ac-testid")).toBe("rootAgent.menu.repo.0.browse.flyout");
      const item = q("rootAgent.menu.repo.0.browse.main")!;
      expect(item.getAttribute("class")).toBe("session-context-option");
      expect(item.textContent).toBe("Open main");
      expect(item.getAttribute("data-ac-role")).toBe("menuitem");
    });

    it("inlineExpand: telegram bot rows render directly under the toggle", () => {
      mount({ telegram: telegramSpecOf(false, { bots: [BOT_1, BOT_2] }) });
      const toggle = q("rootAgent.menu.telegram")!;
      const rowA = q("rootAgent.menu.telegram.bot.b1")!;
      const rowB = q("rootAgent.menu.telegram.bot.b2")!;
      expect(rowA).not.toBeNull();
      expect(rowB).not.toBeNull();
      expect(toggle.nextElementSibling).toBe(rowA);
      expect(rowA.nextElementSibling).toBe(rowB);
      expect(rowA.getAttribute("class")).toBe("session-context-option");
      expect(labelOf(rowA)).toBe("Ops bot");
      expect(rowA.getAttribute("data-ac-role")).toBe("menuitem");
      const dot = rowA.querySelector<HTMLElement>(".session-context-option-icon .settings-color-dot");
      expect(dot).not.toBeNull();
      expect(dot!.style.background).toBe("rgb(255, 0, 0)");
    });

    it("inlineEditor: the editor renders input, Save and Cancel, and the error row outside it", () => {
      mount({ editTaskTitle: titleSpec({ editing: true, draft: "Hello", error: "boom" }) });
      const button = q("rootAgent.menu.editTaskTitle")!;
      expect(button.getAttribute("class")).toBe("session-context-option");
      expect(labelOf(button)).toBe("Edit TASK title");
      assertIcon(button, { kind: "emoji", text: "\u270E" });
      expect(button.getAttribute("data-ac-role")).toBe("menuitem");
      const editor = container().querySelector(".session-context-title-edit")!;
      expect(editor).not.toBeNull();
      const input = editor.querySelector<HTMLInputElement>("input.session-context-title-input")!;
      expect(input.value).toBe("Hello");
      expect(editor.querySelector("button.session-context-title-btn.save")?.textContent).toBe("Save");
      expect(editor.querySelector("button.session-context-title-btn.cancel")?.textContent).toBe("Cancel");
      const error = container().querySelector(".session-context-title-error")!;
      expect(error.textContent).toBe("boom");
      expect(editor.contains(error)).toBe(false);
    });
  });

  // 2. A gate test per item id: with `false` the testid is absent AND the
  //    container is present; with a spec, the item is present.
  it.each(ALL_IDS)("gate: %s is absent when false and present when a spec is given", (id) => {
    const row = EXPECTED.find((r) => r.id === id)!;
    const off = mount({ [id]: false } as SessionRowMenuCaps);
    expect(container()).not.toBeNull();
    expect(q(row.testid)).toBeNull();
    off.dispose();
    mounted = mounted.filter((m) => m !== off);
    document.body.replaceChildren();
    mount(specFor(id));
    expect(container()).not.toBeNull();
    expect(q(row.testid)).not.toBeNull();
  });

  // 3. Order, read synchronously.
  it("renders all twelve items in catalogue order", () => {
    mount(allOn());
    const ids = Array.from(container().querySelectorAll('[data-ac-role="menuitem"]')).map((el) =>
      el.getAttribute("data-ac-testid"),
    );
    expect(ids).toEqual(EXPECTED.map((row) => row.testid));
  });

  // 4. Per-item metadata, table-driven, both toggle states.
  it.each(EXPECTED)("metadata: $id has the literal label, class, icon and danger", (row) => {
    mount(specFor(row.id, true));
    const el = q(row.testid)!;
    expect(el).not.toBeNull();
    expect(labelOf(el)).toBe(row.label);
    expect(el.getAttribute("class")).toBe(row.class);
    assertIcon(el, row.icon);
    expect(el.classList.contains("context-option-danger")).toBe(row.danger);
    expect(el.getAttribute("data-ac-role")).toBe("menuitem");

    if (row.labelOff !== undefined) {
      mount(specFor(row.id, false));
      const all = document.querySelectorAll(`[data-ac-testid="${row.testid}"]`);
      expect(all).toHaveLength(2);
      const off = all[1];
      expect(labelOf(off)).toBe(row.labelOff);
      expect(off.getAttribute("class")).toBe(row.class);
      assertIcon(off, row.iconOff!);
    }
  });

  it("telegram keeps TelegramIcon in both states and changes only its tint", () => {
    mount({ telegram: telegramSpecOf(true, { bridgeColor: "#4ade80" }) });
    const on = iconOf(q("rootAgent.menu.telegram")!) as HTMLElement;
    expect(on.querySelector("svg path")?.getAttribute("d")).toContain(TELEGRAM_D);
    expect(on.style.color).toBe("rgb(74, 222, 128)");
    mount({ telegram: telegramSpecOf(false, { bridgeColor: null }) });
    const both = document.querySelectorAll('[data-ac-testid="rootAgent.menu.telegram"]');
    const off = iconOf(both[1]) as HTMLElement;
    expect(off.querySelector("svg path")?.getAttribute("d")).toContain(TELEGRAM_D);
    expect(off.style.color).toBe("rgb(0, 136, 204)");
  });

  // 5. Separators: positional, between adjacent rendered groups only.
  describe("separators", () => {
    const sequence = (): string[] =>
      Array.from(container().children).map((el) =>
        el.classList.contains("context-separator")
          ? "|"
          : (el.getAttribute("data-ac-testid") ?? `<${el.className}>`),
      );

    it("all five groups on -> exactly four, each between two groups", () => {
      mount(allOn());
      expect(sequence()).toEqual([
        "rootAgent.restart",
        "rootAgent.codingAgent",
        "rootAgent.openFolder",
        "rootAgent.menu.repo.0",
        "rootAgent.menu.matrixFolder",
        "rootAgent.close",
        "|",
        "agent.action.delete.dev-webpage-ui",
        "|",
        "rootAgent.menu.detachToggle",
        "rootAgent.menu.telegram",
        "|",
        "replica.wg-1-dev-team.groups.trigger",
        "|",
        "rootAgent.menu.editTaskTitle",
        "rootAgent.menu.clearTaskTitle",
      ]);
      expect(container().querySelectorAll(".context-separator")).toHaveLength(4);
    });

    it("only G1 on -> zero", () => {
      mount(groupsOn([1]));
      expect(container().querySelectorAll(".context-separator")).toHaveLength(0);
    });

    it("G1 and G3 on with G2 off -> exactly one", () => {
      mount(groupsOn([1, 3]));
      expect(sequence()).toEqual([
        "rootAgent.restart",
        "rootAgent.codingAgent",
        "rootAgent.openFolder",
        "rootAgent.menu.repo.0",
        "rootAgent.menu.matrixFolder",
        "rootAgent.close",
        "|",
        "rootAgent.menu.detachToggle",
        "rootAgent.menu.telegram",
      ]);
    });

    it("G1 with a disabled restart as its only item and G3 on -> one, because disabled is not absent", () => {
      mount({ restart: { onSelect: noop, disabled: true }, ...specFor("detach") });
      expect(sequence()).toEqual(["rootAgent.restart", "|", "rootAgent.menu.detachToggle"]);
    });

    it("nothing on -> zero, and the container is still present and empty", () => {
      mount({});
      expect(container()).not.toBeNull();
      expect(container().children).toHaveLength(0);
      expect(container().querySelectorAll(".context-separator")).toHaveLength(0);
    });
  });

  // 6. Testid derivation.
  it("derives the three preserved rootAgent testids byte-identically", () => {
    mount({ ...specFor("restart"), ...specFor("detach") });
    expect(document.querySelector('[data-ac-testid="rootAgent.menu"]')).not.toBeNull();
    expect(document.querySelector('[data-ac-testid="rootAgent.restart"]')).not.toBeNull();
    expect(document.querySelector('[data-ac-testid="rootAgent.menu.detachToggle"]')).not.toBeNull();
  });

  it("derives the phase-2 session prefix", () => {
    mount({ ...specFor("restart"), ...specFor("detach") }, { prefix: "session.abc" });
    expect(document.querySelector('[data-ac-testid="session.abc.menu"]')).not.toBeNull();
    expect(document.querySelector('[data-ac-testid="session.abc.restart"]')).not.toBeNull();
    expect(document.querySelector('[data-ac-testid="session.abc.menu.detachToggle"]')).not.toBeNull();
  });

  it("derives the repo entry and the browse arrow testids", () => {
    mount({
      repos: reposSpecOf([REPO_A], {
        browseItems: () => [{ id: "main", label: "Open main", url: "https://example.test/main" }],
      }),
    });
    expect(document.querySelector('[data-ac-testid="rootAgent.menu.repo.0"]')).not.toBeNull();
    expect(document.querySelector('[data-ac-testid="rootAgent.menu.repo.0.browse.arrow"]')).not.toBeNull();
  });

  // 7. No duplicate testid.
  it("emits no duplicate testid with all twelve on", () => {
    mount(allOn());
    const ids = Array.from(container().querySelectorAll("[data-ac-testid]")).map((el) =>
      el.getAttribute("data-ac-testid"),
    );
    expect(ids.length).toBeGreaterThanOrEqual(12);
    expect(new Set(ids).size).toBe(ids.length);
  });

  // 8. data-ac-state literals.
  it("emits the four data-ac-state literals", () => {
    mount({ detach: { on: true, onSelect: noop } });
    expect(q("rootAgent.menu.detachToggle")!.getAttribute("data-ac-state")).toBe("detached");
    mount({ detach: { on: false, onSelect: noop } }, { prefix: "b" });
    expect(q("b.menu.detachToggle")!.getAttribute("data-ac-state")).toBe("attached");
    mount({ telegram: telegramSpecOf(true) }, { prefix: "c" });
    expect(q("c.menu.telegram")!.getAttribute("data-ac-state")).toBe("bridged");
    mount({ telegram: telegramSpecOf(false) }, { prefix: "d" });
    expect(q("d.menu.telegram")!.getAttribute("data-ac-state")).toBe("unbridged");
  });

  // 9. Dimming is per item.
  it("dims only clearTaskTitle when disabled; a disabled restart keeps its exact class", () => {
    mount({ restart: { onSelect: noop, disabled: true }, clearTaskTitle: { onSelect: noop, disabled: true } });
    const restart = q("rootAgent.restart") as HTMLButtonElement;
    expect(restart.getAttribute("class")).toBe("session-context-option context-option-danger");
    expect(restart.disabled).toBe(true);
    const clear = q("rootAgent.menu.clearTaskTitle") as HTMLButtonElement;
    expect(clear.getAttribute("class")).toBe("session-context-option context-option-disabled");
    expect(clear.disabled).toBe(true);
  });

  // 10. Dismissal precedes selection; the five exemptions withhold the dismiss
  //     while their own callback still fires exactly once.
  describe("dismiss, then select", () => {
    const dismissingItems: Array<{ id: SessionRowMenuItemId; testid: string }> = [
      { id: "restart", testid: "rootAgent.restart" },
      { id: "codingAgent", testid: "rootAgent.codingAgent" },
      { id: "openFolder", testid: "rootAgent.openFolder" },
      { id: "matrixFolder", testid: "rootAgent.menu.matrixFolder" },
      { id: "close", testid: "rootAgent.close" },
      { id: "deleteAgent", testid: "agent.action.delete.dev-webpage-ui" },
      { id: "clearTaskTitle", testid: "rootAgent.menu.clearTaskTitle" },
      { id: "detach", testid: "rootAgent.menu.detachToggle" },
    ];

    it.each(dismissingItems)("$id dismisses first, then selects", ({ id, testid }) => {
      const order: string[] = [];
      const onSelect = () => order.push("select");
      const caps: SessionRowMenuCaps =
        id === "deleteAgent"
          ? { deleteAgent: { onSelect, testId: testid } }
          : id === "detach"
            ? { detach: { on: false, onSelect } }
            : ({ [id]: { onSelect } } as SessionRowMenuCaps);
      mount(caps, { onDismiss: () => order.push("dismiss") });
      click(q(testid)!);
      expect(order).toEqual(["dismiss", "select"]);
    });

    it("a repo entry dismisses first, then opens the repo", () => {
      const order: string[] = [];
      mount(
        { repos: reposSpecOf([REPO_A], { onOpenRepo: () => order.push("select") }) },
        { onDismiss: () => order.push("dismiss") },
      );
      click(q("rootAgent.menu.repo.0")!);
      expect(order).toEqual(["dismiss", "select"]);
    });

    it("a browse item dismisses first, then opens the browse url", () => {
      const order: string[] = [];
      mount(
        {
          repos: reposSpecOf([REPO_A], {
            browseItems: () => [{ id: "main", label: "Open main", url: "https://example.test/main" }],
            onOpenBrowse: () => order.push("select"),
          }),
        },
        { onDismiss: () => order.push("dismiss") },
      );
      mouseEnter(q("rootAgent.menu.repo.0")!);
      click(q("rootAgent.menu.repo.0.browse.main")!);
      expect(order).toEqual(["dismiss", "select"]);
    });

    it("exempt: the telegram toggle fires onSelect once and never dismisses", () => {
      const onDismiss = vi.fn();
      const onSelect = vi.fn();
      mount({ telegram: telegramSpecOf(false, { onSelect }) }, { onDismiss });
      click(q("rootAgent.menu.telegram")!);
      expect(onSelect).toHaveBeenCalledTimes(1);
      expect(onDismiss).toHaveBeenCalledTimes(0);
    });

    it("exempt: a telegram bot row fires onSelectBot once with its id and never dismisses", () => {
      // The consequence if this is got wrong: the host's onDismiss is closeMenu,
      // whose advanceMenuEpoch() clears the bot list, so a component-side
      // dismiss makes the host's guard reject and the attach is never reached.
      const onDismiss = vi.fn();
      const onSelectBot = vi.fn();
      mount({ telegram: telegramSpecOf(false, { bots: [BOT_1, BOT_2], onSelectBot }) }, { onDismiss });
      click(q("rootAgent.menu.telegram.bot.b2")!);
      expect(onSelectBot).toHaveBeenCalledTimes(1);
      expect(onSelectBot).toHaveBeenCalledWith("b2");
      expect(onDismiss).toHaveBeenCalledTimes(0);
    });

    it("exempt: a group choice fires onToggle once and never dismisses", () => {
      const onDismiss = vi.fn();
      const onToggle = vi.fn();
      mount({ addToGroup: groupSpec({ onToggle }) }, { onDismiss });
      click(q(GROUP_TEST_IDS.trigger)!);
      click(q(GROUP_TEST_IDS.choice("g1"))!);
      expect(onToggle).toHaveBeenCalledTimes(1);
      expect(onToggle).toHaveBeenCalledWith("g1");
      expect(onDismiss).toHaveBeenCalledTimes(0);
    });

    it("exempt: the inline Create row fires create.onSave once and never dismisses", () => {
      const onDismiss = vi.fn();
      const onSave = vi.fn();
      mount(
        { addToGroup: groupSpec({ create: { active: true, draft: "New", onDraft: noop, onStart: noop, onSave } }) },
        { onDismiss },
      );
      click(q(GROUP_TEST_IDS.trigger)!);
      click(q(GROUP_TEST_IDS.createSave)!);
      expect(onSave).toHaveBeenCalledTimes(1);
      expect(onDismiss).toHaveBeenCalledTimes(0);
    });

    it("exempt: Edit TASK title, Save and Cancel each fire their own callback once and never dismiss", () => {
      const onDismiss = vi.fn();
      const onStart = vi.fn();
      const onSave = vi.fn();
      const onCancel = vi.fn();
      mount({ editTaskTitle: titleSpec({ editing: true, draft: "Title", onStart, onSave, onCancel }) }, { onDismiss });
      click(q("rootAgent.menu.editTaskTitle")!);
      expect(onStart).toHaveBeenCalledTimes(1);
      click(container().querySelector(".session-context-title-btn.save")!);
      expect(onSave).toHaveBeenCalledTimes(1);
      click(container().querySelector(".session-context-title-btn.cancel")!);
      expect(onCancel).toHaveBeenCalledTimes(1);
      expect(onDismiss).toHaveBeenCalledTimes(0);
    });
  });

  // 11. Callback identity and payload.
  describe("callback identity and payload", () => {
    it("a repo entry calls onOpenRepo once with that entry's exact sourcePath", () => {
      const onOpenRepo = vi.fn();
      mount({ repos: reposSpecOf([REPO_A, REPO_B], { onOpenRepo }) });
      click(q("rootAgent.menu.repo.1")!);
      expect(onOpenRepo).toHaveBeenCalledTimes(1);
      expect(onOpenRepo).toHaveBeenCalledWith(REPO_B.sourcePath);
    });

    it("a browse item calls onOpenBrowse with that item's exact url", () => {
      const onOpenBrowse = vi.fn();
      mount({
        repos: reposSpecOf([REPO_A], {
          browseItems: () => [
            { id: "main", label: "Open main", url: "https://example.test/main" },
            { id: "branch", label: "Open branch", url: "https://example.test/branch" },
          ],
          onOpenBrowse,
        }),
      });
      mouseEnter(q("rootAgent.menu.repo.0")!);
      click(q("rootAgent.menu.repo.0.browse.branch")!);
      expect(onOpenBrowse).toHaveBeenCalledTimes(1);
      expect(onOpenBrowse).toHaveBeenCalledWith("https://example.test/branch");
    });

    it("a bot row calls onSelectBot with that bot's exact id", () => {
      const onSelectBot = vi.fn();
      mount({ telegram: telegramSpecOf(false, { bots: [BOT_1, BOT_2], onSelectBot }) });
      click(q("rootAgent.menu.telegram.bot.b1")!);
      expect(onSelectBot).toHaveBeenCalledTimes(1);
      expect(onSelectBot).toHaveBeenCalledWith("b1");
    });

    it("a group choice calls onToggle with that choice's exact id", () => {
      const onToggle = vi.fn();
      mount({ addToGroup: groupSpec({ onToggle }) });
      click(q(GROUP_TEST_IDS.trigger)!);
      click(q(GROUP_TEST_IDS.nonstop)!);
      expect(onToggle).toHaveBeenCalledTimes(1);
      expect(onToggle).toHaveBeenCalledWith("nonstop");
    });

    it("close and clearTaskTitle each call their own onSelect and not the neighbour's", () => {
      const onClose = vi.fn();
      const onClear = vi.fn();
      mount({ close: { onSelect: onClose }, clearTaskTitle: { onSelect: onClear } });
      click(q("rootAgent.close")!);
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(onClear).toHaveBeenCalledTimes(0);
      click(q("rootAgent.menu.clearTaskTitle")!);
      expect(onClear).toHaveBeenCalledTimes(1);
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("Save calls onSave and not onCancel; Cancel calls onCancel and not onSave", () => {
      const onSave = vi.fn();
      const onCancel = vi.fn();
      mount({ editTaskTitle: titleSpec({ editing: true, draft: "Title", onSave, onCancel }) });
      click(container().querySelector(".session-context-title-btn.save")!);
      expect(onSave).toHaveBeenCalledTimes(1);
      expect(onCancel).toHaveBeenCalledTimes(0);
      click(container().querySelector(".session-context-title-btn.cancel")!);
      expect(onCancel).toHaveBeenCalledTimes(1);
      expect(onSave).toHaveBeenCalledTimes(1);
    });
  });

  // 12. The two flyout items keep their different click semantics.
  it("clicking a repo trigger opens the repo and not the flyout; clicking Add to Group opens the flyout and calls nothing", () => {
    const onOpenRepo = vi.fn();
    const onToggle = vi.fn();
    const onDismiss = vi.fn();
    mount(
      {
        repos: reposSpecOf([REPO_A], {
          browseItems: () => [{ id: "main", label: "Open main", url: "https://example.test/main" }],
          onOpenRepo,
        }),
        addToGroup: groupSpec({ onToggle }),
      },
      { onDismiss },
    );
    click(q("rootAgent.menu.repo.0")!);
    expect(onOpenRepo).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".session-context-flyout")).toBeNull();
    onDismiss.mockClear();
    click(q(GROUP_TEST_IDS.trigger)!);
    const flyout = document.querySelector(".session-context-flyout");
    expect(flyout).not.toBeNull();
    expect(flyout!.getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.flyout);
    expect(onToggle).toHaveBeenCalledTimes(0);
    expect(onDismiss).toHaveBeenCalledTimes(0);
    expect(onOpenRepo).toHaveBeenCalledTimes(1);
  });

  // 13. The whole addToGroup surface.
  describe("addToGroup surface", () => {
    const openGroupFlyout = (): HTMLElement => {
      click(q(GROUP_TEST_IDS.trigger)!);
      const flyout = q(GROUP_TEST_IDS.flyout);
      if (!flyout) throw new Error("group flyout did not open");
      return flyout;
    };

    it("renders two distinct error rows in order, the second with the menuError testid", () => {
      mount({ addToGroup: groupSpec({ storeError: "store boom", menuError: "menu boom" }) });
      const flyout = openGroupFlyout();
      const errors = flyout.querySelectorAll(".session-context-error");
      expect(errors).toHaveLength(2);
      expect(errors[0].hasAttribute("data-ac-testid")).toBe(false);
      expect(errors[0].textContent).toBe("store boom");
      expect(errors[1].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.menuError);
      expect(errors[1].textContent).toBe("menu boom");
    });

    it("renders exactly one error row when only one is non-null", () => {
      mount({ addToGroup: groupSpec({ storeError: null, menuError: "menu boom" }) });
      const flyout = openGroupFlyout();
      const errors = flyout.querySelectorAll(".session-context-error");
      expect(errors).toHaveLength(1);
      expect(errors[0].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.menuError);
      mount({ addToGroup: groupSpec({ storeError: "store boom", menuError: null, testIds: { ...GROUP_TEST_IDS, trigger: "second.trigger", flyout: "second.flyout" } }) });
      click(q("second.trigger")!);
      const second = q("second.flyout")!;
      const secondErrors = second.querySelectorAll(".session-context-error");
      expect(secondErrors).toHaveLength(1);
      expect(secondErrors[0].hasAttribute("data-ac-testid")).toBe(false);
    });

    it("emits all eight testids, each exactly once", () => {
      const sentinels = {
        trigger: "sentinel.trigger",
        flyout: "sentinel.flyout",
        nonstop: "sentinel.nonstop",
        choice: (id: string) => `sentinel.choice.${id}`,
        create: "sentinel.create",
        createInput: "sentinel.create.input",
        createSave: "sentinel.create.save",
        menuError: "sentinel.error",
      };
      const handle = mount({ addToGroup: groupSpec({ testIds: sentinels, menuError: "boom" }) });
      click(q("sentinel.trigger")!);
      const count = (testid: string) => document.querySelectorAll(`[data-ac-testid="${testid}"]`).length;
      expect(count("sentinel.trigger")).toBe(1);
      expect(count("sentinel.flyout")).toBe(1);
      expect(count("sentinel.nonstop")).toBe(1);
      expect(count("sentinel.choice.g1")).toBe(1);
      expect(count("sentinel.create")).toBe(1);
      expect(count("sentinel.error")).toBe(1);
      handle.setCaps({
        addToGroup: groupSpec({
          testIds: sentinels,
          menuError: "boom",
          create: { active: true, draft: "", onDraft: noop, onStart: noop, onSave: noop },
        }),
      });
      expect(count("sentinel.create.input")).toBe(1);
      expect(count("sentinel.create.save")).toBe(1);
      expect(count("sentinel.create")).toBe(0);
      // Section 7.6: every item button carries data-ac-role="menuitem", the
      // flyout's choice and create buttons included.
      expect(q("sentinel.nonstop")!.getAttribute("data-ac-role")).toBe("menuitem");
      expect(q("sentinel.choice.g1")!.getAttribute("data-ac-role")).toBe("menuitem");
      expect(q("sentinel.create.save")!.getAttribute("data-ac-role")).toBe("menuitem");
    });

    it("every choice button and both create buttons carry data-ac-role=menuitem", () => {
      const handle = mount({ addToGroup: groupSpec() });
      const flyout = openGroupFlyout();
      const choices = flyout.querySelectorAll("button.session-context-group-option");
      expect(choices).toHaveLength(2);
      for (const button of Array.from(choices)) {
        expect(button.getAttribute("data-ac-role")).toBe("menuitem");
      }
      expect(q(GROUP_TEST_IDS.create)!.getAttribute("data-ac-role")).toBe("menuitem");
      handle.setCaps({
        addToGroup: groupSpec({ create: { active: true, draft: "", onDraft: noop, onStart: noop, onSave: noop } }),
      });
      expect(q(GROUP_TEST_IDS.createSave)!.getAttribute("data-ac-role")).toBe("menuitem");
    });

    it("renders checked, disabled and pinned choices as the live surface does", () => {
      const onToggle = vi.fn();
      const choices: GroupChoice[] = [
        { id: "nonstop", name: "Non-stop", checked: false, disabled: false, title: "pinned", pinned: true },
        { id: "g1", name: "Group 1", checked: true, disabled: false, title: "one" },
        { id: "g2", name: "Group 2", checked: false, disabled: true, title: "two" },
      ];
      mount({ addToGroup: groupSpec({ choices, onToggle }) });
      const flyout = openGroupFlyout();
      const buttons = flyout.querySelectorAll<HTMLButtonElement>("button.session-context-group-option");
      expect(buttons).toHaveLength(3);
      expect(buttons[0].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.nonstop);
      expect(buttons[0].classList.contains("session-context-group-option-nonstop")).toBe(true);
      expect(buttons[1].classList.contains("session-context-group-option-nonstop")).toBe(false);
      expect(buttons[1].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.choice("g1"));
      expect(buttons[2].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.choice("g2"));
      expect(buttons[1].querySelector(".session-context-option-check")?.textContent).toBe("\u2713");
      expect(buttons[0].querySelector(".session-context-option-check")?.textContent).toBe("");
      expect(buttons[2].disabled).toBe(true);
      expect(buttons[2].classList.contains("context-option-disabled")).toBe(true);
      expect(buttons[1].classList.contains("context-option-disabled")).toBe(false);
      click(buttons[2]);
      expect(onToggle).toHaveBeenCalledTimes(0);
      click(buttons[1]);
      expect(onToggle).toHaveBeenCalledWith("g1");
    });

    it("renders the emptyNote only for an empty choices array", () => {
      mount({ addToGroup: groupSpec({ choices: [], emptyNote: "No groups yet" }) });
      const flyout = openGroupFlyout();
      const note = flyout.querySelector(".session-context-note");
      expect(note).not.toBeNull();
      expect(note!.textContent).toBe("No groups yet");
      mount({ addToGroup: groupSpec({ emptyNote: "No groups yet", testIds: { ...GROUP_TEST_IDS, trigger: "second.trigger", flyout: "second.flyout" } }) });
      click(q("second.trigger")!);
      expect(q("second.flyout")!.querySelector(".session-context-note")).toBeNull();
    });

    it("drives the create row: onStart, the input, onDraft, Enter and the Create button", () => {
      const onStart = vi.fn();
      const onDraft = vi.fn();
      const onSave = vi.fn();
      const handle = mount({
        addToGroup: groupSpec({ create: { active: false, draft: "", onDraft, onStart, onSave } }),
      });
      openGroupFlyout();
      click(q(GROUP_TEST_IDS.create)!);
      expect(onStart).toHaveBeenCalledTimes(1);
      handle.setCaps({
        addToGroup: groupSpec({ create: { active: true, draft: "Draft", onDraft, onStart, onSave } }),
      });
      expect(q(GROUP_TEST_IDS.create)).toBeNull();
      const input = q(GROUP_TEST_IDS.createInput) as HTMLInputElement;
      expect(input.value).toBe("Draft");
      typeInto(input, "Ops");
      expect(onDraft).toHaveBeenCalledWith("Ops");
      keyDown(input, "Enter");
      expect(onSave).toHaveBeenCalledTimes(1);
      click(q(GROUP_TEST_IDS.createSave)!);
      expect(onSave).toHaveBeenCalledTimes(2);
    });

    it("binds each choice's own title, the only channel the disabled reasons have", () => {
      const choices: GroupChoice[] = [
        { id: "nonstop", name: "Non-stop", checked: false, disabled: false, title: "title-sentinel-nonstop" },
        { id: "g1", name: "Group 1", checked: false, disabled: true, title: "title-sentinel-one" },
        { id: "g2", name: "Group 2", checked: true, disabled: false, title: "title-sentinel-two" },
      ];
      mount({ addToGroup: groupSpec({ choices }) });
      openGroupFlyout();
      expect(q(GROUP_TEST_IDS.nonstop)!.getAttribute("title")).toBe("title-sentinel-nonstop");
      expect(q(GROUP_TEST_IDS.choice("g1"))!.getAttribute("title")).toBe("title-sentinel-one");
      expect(q(GROUP_TEST_IDS.choice("g2"))!.getAttribute("title")).toBe("title-sentinel-two");
    });
  });

  // 14. The whole editTaskTitle surface.
  describe("editTaskTitle surface", () => {
    it("shape: error renders outside the editor; busy and blank drafts disable the right controls", () => {
      mount({ editTaskTitle: titleSpec({ editing: false, error: "boom" }) });
      expect(container().querySelector(".session-context-title-error")?.textContent).toBe("boom");
      expect(container().querySelector("input.session-context-title-input")).toBeNull();

      mount({ editTaskTitle: titleSpec({ editing: true, draft: "Title", busy: true }) }, { prefix: "busy" });
      const busy = q("busy.menu")!;
      expect(busy.querySelector<HTMLInputElement>("input.session-context-title-input")!.disabled).toBe(true);
      expect(busy.querySelector<HTMLButtonElement>(".session-context-title-btn.save")!.disabled).toBe(true);
      expect(busy.querySelector<HTMLButtonElement>(".session-context-title-btn.cancel")!.disabled).toBe(true);

      mount({ editTaskTitle: titleSpec({ editing: true, draft: "   ", busy: false }) }, { prefix: "blank" });
      const blank = q("blank.menu")!;
      expect(blank.querySelector<HTMLInputElement>("input.session-context-title-input")!.disabled).toBe(false);
      expect(blank.querySelector<HTMLButtonElement>(".session-context-title-btn.save")!.disabled).toBe(true);
      expect(blank.querySelector<HTMLButtonElement>(".session-context-title-btn.cancel")!.disabled).toBe(false);
    });

    it("behaviour: onStart, onDraft, Enter, busy Enter, Escape cancels without dismissing", async () => {
      const onDismiss = vi.fn();
      const onStart = vi.fn();
      const onDraft = vi.fn();
      const onSave = vi.fn();
      const onCancel = vi.fn();
      const handle = mount(
        { editTaskTitle: titleSpec({ editing: false, onStart, onDraft, onSave, onCancel }) },
        { onDismiss },
      );
      // Let the surface register its window keydown listener, so the Escape
      // assertion below is a real one: the listener is live and never sees it.
      await hop();
      click(q("rootAgent.menu.editTaskTitle")!);
      expect(onStart).toHaveBeenCalledTimes(1);

      handle.setCaps({ editTaskTitle: titleSpec({ editing: true, draft: "Title", onStart, onDraft, onSave, onCancel }) });
      const input = container().querySelector<HTMLInputElement>("input.session-context-title-input")!;
      typeInto(input, "Renamed");
      expect(onDraft).toHaveBeenCalledWith("Renamed");
      keyDown(input, "Enter");
      expect(onSave).toHaveBeenCalledTimes(1);

      handle.setCaps({ editTaskTitle: titleSpec({ editing: true, draft: "Title", busy: true, onStart, onDraft, onSave, onCancel }) });
      keyDown(container().querySelector<HTMLInputElement>("input.session-context-title-input")!, "Enter");
      expect(onSave).toHaveBeenCalledTimes(1);

      handle.setCaps({ editTaskTitle: titleSpec({ editing: true, draft: "Title", busy: false, onStart, onDraft, onSave, onCancel }) });
      keyDown(container().querySelector<HTMLInputElement>("input.session-context-title-input")!, "Escape");
      expect(onCancel).toHaveBeenCalledTimes(1);
      expect(onDismiss).toHaveBeenCalledTimes(0);
      // The listener really was live: an Escape that nothing stops dismisses.
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });

    it("the input takes focus on mount through requestAnimationFrame", async () => {
      const frames = installDeterministicAnimationFrames();
      try {
        mount({ editTaskTitle: titleSpec({ editing: true, draft: "Title" }) });
        const input = container().querySelector<HTMLInputElement>("input.session-context-title-input")!;
        expect(document.activeElement).not.toBe(input);
        await frames.flush();
        expect(document.activeElement).toBe(input);
      } finally {
        frames.restore();
      }
    });
  });
});
