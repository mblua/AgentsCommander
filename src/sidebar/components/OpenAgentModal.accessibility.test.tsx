// @vitest-environment jsdom
// #4945 commit 1: characterization of the current OpenAgentModal navigation.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component } from "solid-js";
import OpenAgentModal from "./OpenAgentModal";
import type { RepoMatch } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  renderWithFakeTransport,
  waitFor,
} from "../../shared/testing/ui-harness";

type ModalComponent = Component<{ onClose: () => void; initialRepo?: RepoMatch }>;

const SELECTED_REPO_EVENT = "issue-4945:selected-repo";

const FIRST_REPO: RepoMatch = {
  name: "org/alpha",
  path: "C:/issue-4945/repos/alpha",
  agents: ["claude"],
};
const SECOND_REPO: RepoMatch = {
  name: "beta",
  path: "C:/issue-4945/repos/beta",
  agents: ["codex", "claude"],
};

const cleanups: Array<() => void> = [];

beforeEach(() => {
  document.body.innerHTML = "";
});

afterEach(() => {
  while (cleanups.length > 0) cleanups.pop()!();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

function fakeWithRepos(): FakeTransport {
  const fake = new FakeTransport();
  fake.onInvoke("search_repos", () => [FIRST_REPO, SECOND_REPO]);
  fake.onInvoke("get_settings", () => baseSettings());
  return fake;
}

function rows(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>(".agent-modal-item"));
}

function search(): HTMLInputElement {
  const el = document.querySelector<HTMLInputElement>(".agent-search-input");
  if (!el) throw new Error("Search input not mounted");
  return el;
}

function footerEsc(): string {
  const spans = document.querySelectorAll(".agent-modal-footer span");
  return spans[spans.length - 1]?.textContent ?? "";
}

function highlighted(): number[] {
  return rows().flatMap((row, i) => (row.classList.contains("highlighted") ? [i] : []));
}

function key(target: Element, keyName: string): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key: keyName,
    bubbles: true,
    cancelable: true,
  });
  target.dispatchEvent(event);
  return event;
}

function hover(row: Element): void {
  row.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false, cancelable: true }));
}

async function mount(Modal: ModalComponent, initialRepo?: RepoMatch) {
  const onClose = vi.fn();
  const fake = fakeWithRepos();
  const rendered = renderWithFakeTransport(
    () => <Modal onClose={onClose} initialRepo={initialRepo} />,
    fake,
  );
  cleanups.push(rendered.cleanup);
  return { onClose, fake, root: rendered.root };
}

async function mountSettled(Modal: ModalComponent) {
  const mounted = await mount(Modal);
  // Initial onMount search plus the debounced effect search must both settle.
  await waitFor(() => {
    expect(mounted.fake.callsFor("search_repos").length).toBeGreaterThanOrEqual(2);
    expect(rows()).toHaveLength(2);
  });
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(document.activeElement).toBe(search());
  return mounted;
}

async function expectSearchRefocused(): Promise<void> {
  await waitFor(() => expect(document.activeElement).toBe(search()));
}

async function exerciseNavigation(Modal: ModalComponent, observed?: Array<string | null>) {
  const { onClose } = await mountSettled(Modal);
  const [first, second] = rows();
  expect(first).not.toBe(second);
  expect(first!.textContent).toContain("alpha");
  expect(second!.textContent).toContain("beta");
  expect(highlighted()).toEqual([0]);

  hover(second!);
  expect(highlighted()).toEqual([1]);
  hover(first!);
  expect(highlighted()).toEqual([0]);

  click(second!);
  expect(highlighted()).toEqual([0]);
  if (observed) expect(observed.at(-1)).toBe(SECOND_REPO.path);
  // Baseline: the component body is not reactive to selectedRepo, so the repo list
  // stays mounted (no AgentPicker) and only the footer reflects the selection.
  expect(rows()).toHaveLength(2);
  expect(document.querySelector(".agent-picker-modal")).toBeNull();
  expect(footerEsc()).toBe("esc back");

  key(document.activeElement ?? document.body, "Escape");
  expect(onClose).not.toHaveBeenCalled();
  expect(footerEsc()).toBe("esc close");
  if (observed) expect(observed.at(-1)).toBeNull();
  await expectSearchRefocused();

  key(document.activeElement!, "Escape");
  expect(onClose).toHaveBeenCalledTimes(1);
}

describe("OpenAgentModal accessibility baseline (#4945)", () => {
  it("hover moves the highlight between distinct rows", async () => {
    await mountSettled(OpenAgentModal);
    const [first, second] = rows();
    expect(first).not.toBe(second);
    expect(first!.contains(second!)).toBe(false);
    hover(second!);
    expect(highlighted()).toEqual([1]);
    expect(second!.classList.contains("highlighted")).toBe(true);
    expect(first!.classList.contains("highlighted")).toBe(false);
    hover(first!);
    expect(highlighted()).toEqual([0]);
  });

  it("Search ArrowDown/ArrowUp clamp to the list bounds", async () => {
    await mountSettled(OpenAgentModal);
    const input = search();
    const up = key(input, "ArrowUp");
    expect(up.defaultPrevented).toBe(true);
    expect(highlighted()).toEqual([0]);
    expect(key(input, "ArrowDown").defaultPrevented).toBe(true);
    expect(highlighted()).toEqual([1]);
    key(input, "ArrowDown");
    expect(highlighted()).toEqual([1]);
    key(input, "ArrowUp");
    expect(highlighted()).toEqual([0]);
    key(input, "ArrowUp");
    expect(highlighted()).toEqual([0]);
  });

  it("Search Enter selects the highlighted repo and resets highlight", async () => {
    await mountSettled(OpenAgentModal);
    const input = search();
    key(input, "ArrowDown");
    expect(highlighted()).toEqual([1]);
    const enter = key(input, "Enter");
    expect(enter.defaultPrevented).toBe(true);
    expect(highlighted()).toEqual([0]);
    expect(rows()).toHaveLength(2);
    expect(footerEsc()).toBe("esc back");
  });

  it("Search Enter after selection is ignored (no further key handling)", async () => {
    await mountSettled(OpenAgentModal);
    key(search(), "Enter");
    expect(footerEsc()).toBe("esc back");
    const down = key(search(), "ArrowDown");
    expect(down.defaultPrevented).toBe(false);
    expect(highlighted()).toEqual([0]);
  });

  it("click selects the named row, resets highlight, keeps list mounted", async () => {
    await mountSettled(OpenAgentModal);
    const second = rows()[1]!;
    hover(second);
    expect(highlighted()).toEqual([1]);
    click(second);
    expect(highlighted()).toEqual([0]);
    expect(rows()).toHaveLength(2);
    expect(search().isConnected).toBe(true);
    expect(document.querySelector(".agent-picker-modal")).toBeNull();
    expect(footerEsc()).toBe("esc back");
  });

  it("first Escape resets selection and refocuses Search, second closes once", async () => {
    const { onClose } = await mountSettled(OpenAgentModal);
    click(rows()[1]!);
    search().blur();
    expect(document.activeElement).not.toBe(search());
    key(document.body, "Escape");
    expect(onClose).not.toHaveBeenCalled();
    expect(footerEsc()).toBe("esc close");
    await expectSearchRefocused();
    key(document.activeElement!, "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Escape without a selection closes once", async () => {
    const { onClose } = await mountSettled(OpenAgentModal);
    expect(footerEsc()).toBe("esc close");
    key(search(), "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("initialRepo renders AgentPicker directly without the repo list", async () => {
    const { onClose, fake } = await mount(OpenAgentModal, SECOND_REPO);
    expect(document.querySelector(".agent-picker-modal")).not.toBeNull();
    expect(document.querySelector(".agent-search-input")).toBeNull();
    expect(rows()).toHaveLength(0);
    // The OpenAgentModal footer is not rendered; AgentPicker has no esc footer.
    expect(document.querySelector(".agent-modal-footer")).toBeNull();
    expect(document.body.textContent).toContain(SECOND_REPO.name);
    await new Promise((resolve) => setTimeout(resolve, 150));
    expect(fake.callsFor("search_repos")).toHaveLength(1);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("initialRepo Escape closes (esc close semantics)", async () => {
    const { onClose } = await mount(OpenAgentModal, SECOND_REPO);
    key(document.body, "Escape");
    expect(onClose).toHaveBeenCalled();
  });
});

describe("OpenAgentModal P2 selectedRepo observer fixture (#4945)", () => {
  let observed: Array<string | null>;
  let listener: (event: Event) => void;

  beforeEach(() => {
    observed = [];
    listener = (event) => observed.push((event as CustomEvent<string | null>).detail);
    document.addEventListener(SELECTED_REPO_EVENT, listener);
  });

  afterEach(() => {
    document.removeEventListener(SELECTED_REPO_EVENT, listener);
  });

  it("loads a variant module distinct from the ordinary component", async () => {
    const variant = await import("../components/OpenAgentModal");
    expect(variant.default).toBeTypeOf("function");
    expect(variant.default).not.toBe(OpenAgentModal);
  });

  it("reports the second repo's exact path on click and null after Escape", async () => {
    const { default: Variant } = await import("../components/OpenAgentModal");
    await exerciseNavigation(Variant as ModalComponent, observed);
    expect(observed[0]).toBeNull();
    expect(observed).toContain(SECOND_REPO.path);
    expect(observed).not.toContain(FIRST_REPO.path);
  });

  it("ordinary relative mount has the same navigation and emits nothing", async () => {
    await exerciseNavigation(OpenAgentModal);
    expect(observed).toEqual([]);
  });
});

// #4945 phase 2 (P2): rows are native buttons. jsdom has no native button
// activation, so a row keydown is checked as unhandled and the native
// activation is driven by the click it would produce; Chrome covers the keys.
describe("OpenAgentModal phase 2 native row buttons (#4945)", () => {
  it("each row is a type=button named by its repo, with phrasing content only", async () => {
    await mountSettled(OpenAgentModal);
    for (const [row, repo] of [[rows()[0]!, FIRST_REPO], [rows()[1]!, SECOND_REPO]] as const) {
      expect(row.tagName).toBe("BUTTON");
      expect((row as HTMLButtonElement).type).toBe("button");
      expect(row.textContent).toContain(repo.name.slice(repo.name.lastIndexOf("/") + 1));
      expect(row.querySelector("div")).toBeNull();
      row.focus();
      expect(document.activeElement).toBe(row);
    }
  });

  it.each(["Enter", " "])("row %j is left to native activation", async (k) => {
    await mountSettled(OpenAgentModal);
    const second = rows()[1]!;
    second.focus();
    expect(key(second, k).defaultPrevented).toBe(false);
    expect(rows()).toHaveLength(2);
    expect(highlighted()).toEqual([0]);
    expect(footerEsc()).toBe("esc close");
  });

  it("Search Enter still selects the highlighted repo", async () => {
    await mountSettled(OpenAgentModal);
    hover(rows()[1]!);
    expect(key(search(), "Enter").defaultPrevented).toBe(true);
    expect(highlighted()).toEqual([0]);
    expect(footerEsc()).toBe("esc back");
  });

  describe("observer fixture", () => {
    let observed: Array<string | null>;
    const listener = (event: Event) => observed.push((event as CustomEvent<string | null>).detail);
    beforeEach(() => {
      observed = [];
      document.addEventListener(SELECTED_REPO_EVENT, listener);
    });
    afterEach(() => document.removeEventListener(SELECTED_REPO_EVENT, listener));

    it.each(["Enter", " "])("native %j activation of the second row emits its path", async (k) => {
      const { default: Variant } = await import("../components/OpenAgentModal");
      await mountSettled(Variant as ModalComponent);
      const second = rows()[1]!;
      second.focus();
      expect(key(second, k).defaultPrevented).toBe(false);
      second.click();
      expect(observed.at(-1)).toBe(SECOND_REPO.path);
      expect(observed).not.toContain(FIRST_REPO.path);
    });
  });
});
