// @vitest-environment jsdom
// #5287 - ring and message bubble of the "one message to the Root Agent" tutorial.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../../shared/platform", () => ({
  get isTauri() { return platform.tauri; },
  get isBrowser() { return !platform.tauri; },
  isWindows: false,
}));

import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings, click, installBrowserDomStubs, renderWithFakeTransport,
  resetUiStoresForTests, waitFor,
} from "../../shared/testing/ui-harness";
import { SessionAPI } from "../../shared/ipc";
import {
  __disposeSettingsSubscriptionsForTests, settingsStore,
} from "../../shared/stores/settings";
import {
  __resetRootMessageStoreForTests, rootMessageStore,
} from "../stores/root-message-tutorial";
import { __resetTutorialProgressForTests } from "../stores/tutorial-progress";
import {
  COPIED, COPY_FAILED, ROOT_MESSAGE, T_DORMANT, T_LIVE, T_MISSING, T_NO_AGENT, T_NO_BANNER,
} from "../tutorials/root-agent-message";
import TutorialLayer from "./TutorialLayer";

const q = (id: string) => document.body.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
const must = (id: string) => {
  const el = q(id);
  if (!el) throw new Error(`Missing ${id}`);
  return el;
};
const cleanups: Array<() => void> = [];
let frames: FrameRequestCallback[] = [];
const sessionSpies: Array<ReturnType<typeof vi.fn>> = [];

async function flush() {
  await Promise.resolve();
  const run = frames;
  frames = [];
  run.forEach((cb) => cb(0));
  await Promise.resolve();
}
function target(id: string, top: number, state?: string) {
  const el = document.createElement("div");
  el.setAttribute("data-ac-testid", id);
  if (state) el.setAttribute("data-ac-state", state);
  el.getBoundingClientRect = () =>
    ({ top, left: 20, width: 30, height: 40, bottom: top + 40, right: 50 }) as DOMRect;
  document.body.appendChild(el);
  return el;
}
async function mount(agents: number) {
  const f = new FakeTransport();
  const list = Array.from({ length: agents }, (_, i) => ({ id: `a${i}`, label: `A${i}` }));
  f.resolve("get_settings", { ...baseSettings(), tutorialHintDismissed: true, agents: list });
  const r = renderWithFakeTransport(() => <TutorialLayer />, f);
  cleanups.push(r.cleanup);
  await settingsStore.load();
  await waitFor(() => expect(settingsStore.current).toBeTruthy());
  rootMessageStore.start();
  await flush();
}
const text = () => must("rootMessage.bubble").textContent ?? "";
const ringTop = () => must("rootMessage.ring").style.top;

beforeEach(() => {
  platform.tauri = true;
  cleanups.push(installBrowserDomStubs());
  frames = [];
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    frames.push(cb);
    return frames.length;
  });
  vi.stubGlobal("cancelAnimationFrame", () => { frames = []; });
  resetUiStoresForTests();
  __disposeSettingsSubscriptionsForTests();
  __resetTutorialProgressForTests();
  __resetRootMessageStoreForTests();
  sessionSpies.length = 0;
  for (const [k, fn] of Object.entries(SessionAPI)) {
    if (typeof fn === "function") {
      sessionSpies.push(vi.spyOn(SessionAPI, k as never) as never);
    }
  }
});
afterEach(() => {
  for (const s of sessionSpies) expect(s).not.toHaveBeenCalled();
  vi.unstubAllGlobals();
  while (cleanups.length) cleanups.pop()?.();
  resetUiStoresForTests();
  __disposeSettingsSubscriptionsForTests();
  __resetRootMessageStoreForTests();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("RootMessageOverlay", () => {
  it("ring on the banner follows its state", async () => {
    const banner = target("rootAgent.banner", 100, "missing");
    target("actionBar.settings", 300);
    await mount(1);
    expect(ringTop()).toBe("96px");
    expect(text()).toContain(T_MISSING);
    expect(must("rootMessage.text").textContent).toBe(ROOT_MESSAGE);
    banner.setAttribute("data-ac-state", "dormant");
    await flush();
    expect(text()).toContain(T_DORMANT);
    banner.setAttribute("data-ac-state", "live");
    await flush();
    expect(text()).toContain(T_LIVE);
    expect(ringTop()).toBe("96px");
  });

  it("no coding agent: ring on the Settings gear and warn", async () => {
    target("rootAgent.banner", 100, "missing");
    target("actionBar.settings", 300);
    await mount(0);
    expect(ringTop()).toBe("296px");
    expect(text()).toContain(T_NO_AGENT);
    expect(must("rootMessage.bubble").dataset.acState).toBe("warn");
  });

  it("no banner: fallback corner, no ring", async () => {
    await mount(1);
    expect(q("rootMessage.ring")).toBeNull();
    expect(text()).toContain(T_NO_BANNER);
    expect(must("rootMessage.bubble").style.right).toBe("24px");
  });

  it("Copy writes ROOT_MESSAGE and shows Copied", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: write }, configurable: true,
    });
    target("rootAgent.banner", 100, "live");
    await mount(1);
    click(must("rootMessage.copy"));
    await waitFor(() => expect(must("rootMessage.copy").textContent).toBe(COPIED));
    expect(write).toHaveBeenCalledWith(ROOT_MESSAGE);
    expect(q("rootMessage.copyFailed")).toBeNull();
  });

  it("a rejected copy shows the fallback line", async () => {
    const write = vi.fn().mockRejectedValue(new Error("denied"));
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: write }, configurable: true,
    });
    target("rootAgent.banner", 100, "live");
    await mount(1);
    click(must("rootMessage.copy"));
    await waitFor(() => expect(must("rootMessage.copyFailed").textContent).toBe(COPY_FAILED));
  });

  it("Close ends the run", async () => {
    target("rootAgent.banner", 100, "live");
    await mount(1);
    click(must("rootMessage.close"));
    expect(rootMessageStore.active).toBe(false);
    await flush();
    expect(q("rootMessage.bubble")).toBeNull();
  });
});
