// @vitest-environment jsdom
// #5228 - Home tutorials section.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../../shared/platform", () => ({
  get isTauri() { return platform.tauri; },
  get isBrowser() { return !platform.tauri; },
  isWindows: false,
}));

import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings, click, installBrowserDomStubs, registerCompactHostForTests,
  renderWithFakeTransport, resetUiStoresForTests, waitFor,
} from "../../shared/testing/ui-harness";
import { setSidebarCompactMode } from "../../shared/sidebar-compact";
import { sessionsStore } from "../../sidebar/stores/sessions";
import { __resetHomeStoreForTests } from "../stores/home";
import { __resetTutorialStoreForTests, tutorialStore } from "../stores/tutorial";
import {
  __resetRootMessageStoreForTests, rootMessageStore,
} from "../stores/root-message-tutorial";
import {
  __resetTutorialProgressForTests, tutorialProgressStore,
} from "../stores/tutorial-progress";
import HomeView from "./HomeView";
import HomeTutorials from "./HomeTutorials";
import TutorialLayer from "./TutorialLayer";

const q = (id: string) => document.body.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
const must = (id: string) => {
  const el = q(id);
  if (!el) throw new Error(`Missing ${id}`);
  return el;
};
const startBtn = () => must("home.tutorial.team.start") as HTMLButtonElement;
const cleanups: Array<() => void> = [];

function fake(flags: object = {}): FakeTransport {
  const f = new FakeTransport();
  f.resolve("get_settings", { ...baseSettings(), ...flags });
  f.resolve("fetch_home_markdown", "# Home");
  return f;
}
function mount(view: () => unknown, f = fake()) {
  const r = renderWithFakeTransport(view as never, f);
  cleanups.push(r.cleanup);
  return r;
}

beforeEach(() => {
  platform.tauri = true;
  cleanups.push(installBrowserDomStubs());
  resetUiStoresForTests();
  __resetHomeStoreForTests();
  __resetTutorialStoreForTests();
  __resetTutorialProgressForTests();
  __resetRootMessageStoreForTests();
});
afterEach(() => {
  while (cleanups.length) cleanups.pop()?.();
  resetUiStoresForTests();
  if (!sessionsStore.showCategories) sessionsStore.toggleShowCategories();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("HomeTutorials", () => {
  it("renders with failed Markdown (content null + error)", async () => {
    const f = fake();
    f.reject("fetch_home_markdown", "boom");
    mount(() => <HomeView />, f);
    await waitFor(() => expect(document.querySelector(".home-status-error")).toBeTruthy());
    expect(q("home.tutorials")).toBeTruthy();
  });

  it("renders with empty Markdown", async () => {
    const f = fake();
    f.resolve("fetch_home_markdown", "");
    mount(() => <HomeView />, f);
    await waitFor(() => expect(document.querySelector(".home-markdown")).toBeTruthy());
    expect(q("home.tutorials")).toBeTruthy();
  });

  it("renders while Markdown is loading", async () => {
    const f = fake();
    f.onInvoke("fetch_home_markdown", () => new Promise(() => {}));
    mount(() => <HomeView />, f);
    await waitFor(() => expect(document.querySelector(".home-status")).toBeTruthy());
    expect(q("home.tutorials")).toBeTruthy();
  });

  it("is hidden in browser view", () => {
    platform.tauri = false;
    mount(() => <HomeTutorials />);
    expect(q("home.tutorials")).toBeNull();
  });

  it("compact sidebar disables Start with the compact text", () => {
    registerCompactHostForTests();
    setSidebarCompactMode(true);
    mount(() => <HomeTutorials />);
    expect(startBtn().disabled).toBe(true);
    expect(must("home.tutorial.team").textContent)
      .toContain("Expand the sidebar to use this tutorial.");
  });

  it("hidden categories disable Start with the categories text", () => {
    sessionsStore.toggleShowCategories();
    mount(() => <HomeTutorials />);
    expect(startBtn().disabled).toBe(true);
    expect(must("home.tutorial.team").textContent)
      .toContain("Show category sections to use this tutorial.");
  });

  it("completed shows Completed and Start again", async () => {
    mount(() => <HomeTutorials />, fake({ teamTutorialCompleted: true }));
    await tutorialProgressStore.load();
    await waitFor(() => expect(must("home.tutorial.team").dataset.acState).toBe("completed"));
    expect(must("home.tutorial.team").textContent).toContain("Completed");
    expect(startBtn().textContent).toBe("Start again");
  });

  it("never auto-starts; Start starts the run", async () => {
    mount(() => <><HomeTutorials /><TutorialLayer /></>);
    await waitFor(() => expect(tutorialProgressStore.loaded).toBe(true));
    expect(tutorialStore.active).toBe(false);
    expect(startBtn().textContent).toBe("Start");
    click(startBtn());
    expect(tutorialStore.active).toBe(true);
    expect(startBtn().disabled).toBe(true);
  });

  // #5287
  const rmBtn = () => must("home.tutorial.rootMessage.start") as HTMLButtonElement;
  it("two cards; the #5228 card keeps its testids and state", () => {
    mount(() => <HomeTutorials />);
    const cards = document.querySelectorAll(".home-tutorial-cards > .home-tutorial-card");
    expect(cards.length).toBe(2);
    expect(must("home.tutorial.team").dataset.acState).toBe("new");
    expect(startBtn().disabled).toBe(false);
    expect(must("home.tutorial.rootMessage").textContent)
      .toContain("Build the same team with one message to the Root Agent");
    expect(rmBtn().disabled).toBe(false);
  });

  it("root-message Start disables the #5228 Start", () => {
    mount(() => <HomeTutorials />);
    click(rmBtn());
    expect(rootMessageStore.active).toBe(true);
    expect(rmBtn().disabled).toBe(true);
    expect(startBtn().disabled).toBe(true);
  });

  it("#5228 Start disables the root-message Start", () => {
    mount(() => <HomeTutorials />);
    click(startBtn());
    expect(tutorialStore.active).toBe(true);
    expect(rmBtn().disabled).toBe(true);
    expect(rootMessageStore.active).toBe(false);
  });
});
