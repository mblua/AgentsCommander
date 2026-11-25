// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component } from "solid-js";
import OpenAgentModal from "./OpenAgentModal";
import { homeStore } from "../../main/stores/home";
import { SessionAPI } from "../../shared/ipc";
import type { RepoMatch, Session } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  input,
  renderWithFakeTransport,
  session,
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

function fakeWithAgent(): FakeTransport {
  const fake = fakeWithRepos();
  fake.resolve("get_settings", baseSettings({
    agents: [{
      id: "codex",
      label: "Codex",
      command: "codex",
      color: "#10b981",
      envs: [],
      isolatedHome: false,
    }],
  }));
  return fake;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function rows(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>(".agent-modal-item"));
}

function search(): HTMLInputElement {
  const el = document.querySelector<HTMLInputElement>(".agent-search-input");
  if (!el) throw new Error("Search input not mounted");
  return el;
}

async function expectRepoPicker(repo: RepoMatch): Promise<HTMLElement> {
  const picker = document.querySelector<HTMLElement>(".agent-picker-modal");
  if (!picker) throw new Error(`Expected the ${repo.name} agent picker to mount`);
  expect(picker.querySelector(".agent-picker-context span")?.textContent).toBe(repo.name);
  expect(document.querySelector(".agent-search-input")).toBeNull();
  expect(rows()).toHaveLength(0);
  await waitFor(() => {
    expect(picker.closest(".modal-overlay")?.contains(document.activeElement)).toBe(true);
  });
  return picker;
}

function pickerButton(id: string): HTMLButtonElement {
  const button = document.querySelector<HTMLButtonElement>(`[data-ac-testid="${id}"]`);
  if (!button) throw new Error(`Picker button ${id} not mounted`);
  return button;
}

async function applyAgent(): Promise<HTMLButtonElement> {
  const apply = pickerButton("agentPicker.apply");
  await waitFor(() => expect(apply.disabled).toBe(false));
  apply.click();
  return apply;
}

function expectBetaCreate(fake: FakeTransport) {
  const calls = fake.callsFor("create_session");
  expect(calls).toHaveLength(1);
  expect(calls[0]?.args).toMatchObject({
    cwd: SECOND_REPO.path,
    sessionName: SECOND_REPO.name,
    agentId: "codex",
  });
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

async function mount(
  Modal: ModalComponent,
  initialRepo?: RepoMatch,
  { onClose = vi.fn(), fake = fakeWithRepos() } = {},
) {
  const rendered = renderWithFakeTransport(
    () => <Modal onClose={onClose} initialRepo={initialRepo} />,
    fake,
  );
  let mounted = true;
  const unmount = () => {
    if (!mounted) return;
    mounted = false;
    rendered.cleanup();
  };
  cleanups.push(unmount);
  return { onClose, fake, unmount };
}

async function mountSettled(Modal: ModalComponent, fake = fakeWithRepos()) {
  const mounted = await mount(Modal, undefined, { fake });
  // Initial onMount search plus the debounced effect search must both settle.
  await waitFor(() => {
    expect(mounted.fake.callsFor("search_repos").length).toBeGreaterThanOrEqual(2);
    expect(rows()).toHaveLength(2);
    expect(document.querySelector(".agent-search-spinner")).toBeNull();
  });
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
  await expectRepoPicker(SECOND_REPO);
  if (observed) expect(observed.at(-1)).toBe(SECOND_REPO.path);

  key(document.activeElement!, "Escape");
  expect(onClose).not.toHaveBeenCalled();
  expect(footerEsc()).toBe("esc close");
  expect(highlighted()).toEqual([0]);
  if (observed) expect(observed.at(-1)).toBeNull();
  await expectSearchRefocused();

  key(document.activeElement!, "Escape");
  expect(onClose).toHaveBeenCalledTimes(1);
}

describe("OpenAgentModal repo navigation", () => {
  it("row click opens the second repo picker", async () => {
    await mountSettled(OpenAgentModal);
    const second = rows()[1]!;
    expect(second.textContent).toContain(SECOND_REPO.name);
    click(second);
    await expectRepoPicker(SECOND_REPO);
  });

  it("Search Enter opens the second repo picker", async () => {
    await mountSettled(OpenAgentModal);
    const searchInput = search();
    key(searchInput, "ArrowDown");
    expect(highlighted()).toEqual([1]);
    expect(key(searchInput, "Enter").defaultPrevented).toBe(true);
    await expectRepoPicker(SECOND_REPO);
  });

  it("positive control: initialRepo opens the second repo picker directly", async () => {
    await mount(OpenAgentModal, SECOND_REPO);
    await expectRepoPicker(SECOND_REPO);
  });

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
    const searchInput = search();
    const up = key(searchInput, "ArrowUp");
    expect(up.defaultPrevented).toBe(true);
    expect(highlighted()).toEqual([0]);
    expect(key(searchInput, "ArrowDown").defaultPrevented).toBe(true);
    expect(highlighted()).toEqual([1]);
    key(searchInput, "ArrowDown");
    expect(highlighted()).toEqual([1]);
    key(searchInput, "ArrowUp");
    expect(highlighted()).toEqual([0]);
    key(searchInput, "ArrowUp");
    expect(highlighted()).toEqual([0]);
  });

  it("Search Enter resets highlight when returning from the picker", async () => {
    await mountSettled(OpenAgentModal);
    const searchInput = search();
    key(searchInput, "ArrowDown");
    expect(highlighted()).toEqual([1]);
    const enter = key(searchInput, "Enter");
    expect(enter.defaultPrevented).toBe(true);
    await expectRepoPicker(SECOND_REPO);
    key(document.activeElement!, "Escape");
    expect(highlighted()).toEqual([0]);
    expect(rows()).toHaveLength(2);
    await expectSearchRefocused();
  });

  it("the old Search input stops handling list keys after selection", async () => {
    await mountSettled(OpenAgentModal);
    const searchInput = search();
    key(searchInput, "Enter");
    await expectRepoPicker(FIRST_REPO);
    expect(searchInput.isConnected).toBe(false);
    const down = key(searchInput, "ArrowDown");
    expect(down.defaultPrevented).toBe(false);
    await expectRepoPicker(FIRST_REPO);
  });

  it("clicking the hovered row resets highlight on Cancel", async () => {
    await mountSettled(OpenAgentModal);
    const second = rows()[1]!;
    hover(second);
    expect(highlighted()).toEqual([1]);
    click(second);
    await expectRepoPicker(SECOND_REPO);
    pickerButton("agentPicker.cancel").click();
    expect(highlighted()).toEqual([0]);
    expect(rows()).toHaveLength(2);
    await expectSearchRefocused();
  });

  it.each(["picker", "body"])(
    "Escape from %s restores the list and the next Escape closes once",
    async (target) => {
      const fake = fakeWithRepos();
      fake.onInvoke("search_repos", ({ query }) =>
        query === "a" ? [FIRST_REPO, SECOND_REPO] : [SECOND_REPO, FIRST_REPO],
      );
      const { onClose } = await mountSettled(OpenAgentModal, fake);
      const previousSearch = search();
      input(previousSearch, "a");
      await waitFor(() => {
        expect(fake.lastCall("search_repos")?.args.query).toBe("a");
        expect(document.querySelector(".agent-search-spinner")).toBeNull();
      });
      const results = rows().map((row) => row.textContent);
      hover(rows()[1]!);
      expect(highlighted()).toEqual([1]);
      click(rows()[1]!);
      await expectRepoPicker(SECOND_REPO);

      key(target === "picker" ? document.activeElement! : document.body, "Escape");
      expect(onClose).not.toHaveBeenCalled();
      expect(footerEsc()).toBe("esc close");
      expect(rows().map((row) => row.textContent)).toEqual(results);
      expect(highlighted()).toEqual([0]);
      expect(search()).not.toBe(previousSearch);
      expect(search().value).toBe("a");
      await expectSearchRefocused();

      key(search(), "Escape");
      expect(onClose).toHaveBeenCalledTimes(1);
    },
  );

  it("Escape without a selection closes once", async () => {
    const { onClose } = await mountSettled(OpenAgentModal);
    expect(footerEsc()).toBe("esc close");
    key(search(), "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Escape from body closes once while initial search is pending", async () => {
    const fake = fakeWithRepos();
    const pending = deferred<RepoMatch[]>();
    fake.onInvoke("search_repos", () => pending.promise);
    const { onClose, unmount } = await mount(OpenAgentModal, undefined, { fake });
    expect(fake.callsFor("search_repos")).toHaveLength(1);
    expect(key(document.body, "Escape").defaultPrevented).toBe(true);
    expect(onClose).toHaveBeenCalledTimes(1);
    unmount();
    pending.resolve([FIRST_REPO, SECOND_REPO]);
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

  it.each(["picker", "body"])("initialRepo Escape from %s closes once", async (target) => {
    const { onClose } = await mount(OpenAgentModal, SECOND_REPO);
    await expectRepoPicker(SECOND_REPO);
    key(target === "picker" ? document.activeElement! : document.body, "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("OpenAgentModal launch", () => {
  it("applies a valid agent once in the selected second repo", async () => {
    const fake = fakeWithAgent();
    fake.resolve("create_session", session());
    const hideHome = vi.spyOn(homeStore, "hide");
    const { onClose } = await mountSettled(OpenAgentModal, fake);
    click(rows()[1]!);
    await expectRepoPicker(SECOND_REPO);
    await applyAgent();

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expectBetaCreate(fake);
    expect(hideHome).toHaveBeenCalledTimes(1);
  });

  it("holds the same picker on Escape and repeat Apply while create is pending", async () => {
    const fake = fakeWithAgent();
    const pending = deferred<Session>();
    fake.onInvoke("create_session", () => pending.promise);
    const { onClose } = await mountSettled(OpenAgentModal, fake);
    click(rows()[1]!);
    const picker = await expectRepoPicker(SECOND_REPO);
    const apply = await applyAgent();
    expectBetaCreate(fake);
    expect(apply.disabled).toBe(true);
    expect(pickerButton("agentPicker.cancel").disabled).toBe(true);

    for (const target of [document.activeElement!, document.body]) {
      key(target, "Escape");
      expect(await expectRepoPicker(SECOND_REPO)).toBe(picker);
      expect(onClose).not.toHaveBeenCalled();
    }
    apply.click();
    key(picker, "Enter");
    expectBetaCreate(fake);

    pending.resolve(session());
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expectBetaCreate(fake);
  });

  it("restores Apply and Back after a create rejection", async () => {
    const fake = fakeWithAgent();
    const pending = deferred<Session>();
    fake.onInvoke("create_session", () => pending.promise);
    const { onClose } = await mountSettled(OpenAgentModal, fake);
    click(rows()[1]!);
    const picker = await expectRepoPicker(SECOND_REPO);
    const apply = await applyAgent();
    expectBetaCreate(fake);

    pending.reject(new Error("Test create failed"));
    await waitFor(() => {
      expect(picker.querySelector(".agent-picker-error")?.textContent).toBe("Test create failed");
      expect(apply.disabled).toBe(false);
      expect(pickerButton("agentPicker.cancel").disabled).toBe(false);
    });
    key(document.activeElement!, "Escape");
    await expectSearchRefocused();
    click(rows()[0]!);
    await expectRepoPicker(FIRST_REPO);
    expect(onClose).not.toHaveBeenCalled();
    expectBetaCreate(fake);
  });

  it("ignores completion from a direct picker closed during create", async () => {
    const fake = fakeWithAgent();
    const pending = deferred<Session>();
    fake.onInvoke("create_session", () => pending.promise);
    const onClose = vi.fn();
    const hideHome = vi.spyOn(homeStore, "hide");
    const create = vi.spyOn(SessionAPI, "create");
    const beta = await mount(OpenAgentModal, SECOND_REPO, { fake, onClose });
    await expectRepoPicker(SECOND_REPO);
    await applyAgent();
    expectBetaCreate(fake);

    key(document.activeElement!, "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
    beta.unmount();
    await mount(OpenAgentModal, FIRST_REPO, { onClose });
    const alpha = await expectRepoPicker(FIRST_REPO);

    pending.resolve(session());
    await create.mock.results[0]!.value;
    expect(await expectRepoPicker(FIRST_REPO)).toBe(alpha);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(hideHome).not.toHaveBeenCalled();
    expect(create).toHaveBeenCalledTimes(1);
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

// Phase 2 (P2): rows are native buttons. jsdom has no native button
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
    await expectRepoPicker(SECOND_REPO);
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
      await expectRepoPicker(SECOND_REPO);
      expect(observed.at(-1)).toBe(SECOND_REPO.path);
      expect(observed).not.toContain(FIRST_REPO.path);
    });
  });
});
