// @vitest-environment jsdom
// #1871 section 10.5 - the mechanics, black-box through SessionRowMenu so that
// ContextMenuSurface stays private (this file never imports it, which is why
// the import-boundary allowlist has one element).
//
// The timer regime, as three primitives rather than a per-test table:
//   1. flushRegistration(): one flush, correct under either clock.
//   2. openMenu(): the ONE way to open, and it flushes, so every test gets the
//      clamp and the window listeners without reading anything in the plan.
//   3. openMenuWithoutRegistering(): the ONE named opt-out, for assertions that
//      must observe the state BEFORE the registration timer fires. It has
//      exactly two call sites: test 6 parts 1 and 2.
// The clock: installBrowserDomStubs() FIRST, vi.useFakeTimers() AFTER it, so
// fake is the default. useFrameClock() opts a test out onto real timers plus
// the deterministic frame harness; it is the only way to observe a value the
// surface computes inside requestAnimationFrame. The two clocks cannot be
// combined: frames.flush() awaits a setTimeout(0) that fake timers never fire,
// so it hangs to the suite timeout instead of failing.
import { batch, createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SessionRowMenu from "./SessionRowMenu";
import type { AddToGroupSpec, ReposSpec, SessionRowMenuCaps } from "./session-row-menu-types";
import type { SessionRepo } from "../../../shared/types";
import {
  installBrowserDomStubs,
  installDeterministicAnimationFrames,
  type DeterministicAnimationFrames,
} from "../../../shared/testing/ui-harness";

const noop = (): void => {};

const REPO_A: SessionRepo = { label: "AgentsCommander", sourcePath: "D:\\repos\\AgentsCommander", branch: null, dirty: null };
const BROWSE_ITEMS = [{ id: "main" as const, label: "Open main", url: "https://example.test/main" }];

const GROUP_TEST_IDS = {
  trigger: "replica.wg-1.groups.trigger",
  flyout: "replica.wg-1.groups.flyout",
  nonstop: "replica.wg-1.groups.nonstop",
  choice: (id: string) => `replica.wg-1.groups.${id}`,
  create: "replica.wg-1.groups.create",
  createInput: "replica.groups.create.input",
  createSave: "replica.groups.create.save",
  menuError: "replica.groups.error",
};

const reposWithBrowse = (overrides: Partial<ReposSpec> = {}): ReposSpec => ({
  repos: [REPO_A],
  browseItems: () => BROWSE_ITEMS,
  onOpenRepo: noop,
  onOpenBrowse: noop,
  ...overrides,
});

const groupSpec = (): AddToGroupSpec => ({
  choices: [{ id: "nonstop", name: "Non-stop", checked: false, disabled: false, title: "pinned" }],
  onToggle: noop,
  emptyNote: null,
  storeError: null,
  menuError: null,
  create: { active: false, draft: "", onDraft: noop, onStart: noop, onSave: noop },
  testIds: GROUP_TEST_IDS,
});

const BASE_CAPS: SessionRowMenuCaps = { restart: { onSelect: noop } };

const q = (testid: string): HTMLElement | null =>
  document.querySelector<HTMLElement>(`[data-ac-testid="${testid}"]`);
const menuEl = (): HTMLElement => {
  const el = q("rootAgent.menu");
  if (!el) throw new Error("rootAgent.menu did not render");
  return el;
};
const flyouts = (): NodeListOf<HTMLElement> =>
  document.querySelectorAll<HTMLElement>(".session-context-flyout");

// Solid does not delegate mouseenter/mouseleave (they do not bubble); it binds
// them directly, so a non-bubbling dispatch is exactly what the handler sees.
const mouseEnter = (el: Element): boolean =>
  el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false, cancelable: true }));
const mouseLeave = (el: Element): boolean =>
  el.dispatchEvent(new MouseEvent("mouseleave", { bubbles: false, cancelable: true }));
const click = (el: Element): boolean =>
  el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
const keyDown = (el: Element, key: string): boolean =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
const windowEvent = (type: "click" | "contextmenu"): boolean =>
  window.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true }));
const windowKey = (key: string): boolean =>
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

// Primitive 1. The registration timer is a zero-delay setTimeout. Under fake
// timers it is advanced; under real timers it is awaited - a setTimeout(0)
// queued after it fires after it, because equal-delay timers fire in
// insertion order.
const flushRegistration = async (): Promise<void> => {
  if (vi.isFakeTimers()) vi.advanceTimersByTime(0);
  else await new Promise<void>((resolve) => setTimeout(resolve, 0));
};

function domRect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    x: left,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

interface RectStubs {
  menu?: { width: number; height: number };
  anchor?: { left: number; top: number; width: number; height: number };
  flyout?: { width: number; height: number };
}
const rects: RectStubs = {};

function stubRects(): void {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    if (this.classList.contains("session-context-menu") && rects.menu) {
      return domRect(0, 0, rects.menu.width, rects.menu.height);
    }
    if (this.classList.contains("session-context-flyout") && rects.flyout) {
      return domRect(0, 0, rects.flyout.width, rects.flyout.height);
    }
    if (this.classList.contains("session-context-repo-option") && rects.anchor) {
      const a = rects.anchor;
      return domRect(a.left, a.top, a.width, a.height);
    }
    return domRect(0, 0, 0, 0);
  });
}

const originalInnerWidth = window.innerWidth;
const originalInnerHeight = window.innerHeight;
function setViewport(width: number, height: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
  Object.defineProperty(window, "innerHeight", { configurable: true, writable: true, value: height });
}

interface Host {
  setOpen: (open: boolean) => void;
  setPoint: (point: { x: number; y: number }) => void;
  setCaps: (caps: SessionRowMenuCaps) => void;
  onDismiss: ReturnType<typeof vi.fn>;
  dispose: () => void;
}

let host: Host | null = null;
let cleanupDom: (() => void) | null = null;
let frames: DeterministicAnimationFrames | null = null;

function mountHost(): Host {
  const root = document.createElement("div");
  document.body.appendChild(root);
  const [open, setOpen] = createSignal(false);
  const [point, setPoint] = createSignal({ x: 20, y: 30 });
  const [caps, setCaps] = createSignal<SessionRowMenuCaps>(BASE_CAPS);
  const onDismiss = vi.fn();
  const dispose = render(
    () => (
      <SessionRowMenu
        open={open()}
        x={point().x}
        y={point().y}
        testIdPrefix="rootAgent"
        caps={caps()}
        onDismiss={() => {
          onDismiss();
          setOpen(false);
        }}
      />
    ),
    root,
  );
  const handle: Host = {
    setOpen,
    setPoint,
    setCaps,
    onDismiss,
    dispose: () => {
      dispose();
      root.remove();
    },
  };
  host = handle;
  return handle;
}

interface OpenOptions {
  x?: number;
  y?: number;
  caps?: SessionRowMenuCaps;
}

// Primitive 3. Opens and deliberately does NOT flush. Two call sites only:
// test 6 parts 1 and 2.
function openMenuWithoutRegistering(opts: OpenOptions = {}): void {
  const h = host ?? mountHost();
  batch(() => {
    if (opts.caps) h.setCaps(opts.caps);
    h.setPoint({ x: opts.x ?? 20, y: opts.y ?? 30 });
    h.setOpen(true);
  });
}

// Primitive 2. The one way to open; it flushes.
async function openMenu(opts: OpenOptions = {}): Promise<void> {
  openMenuWithoutRegistering(opts);
  await flushRegistration();
}

// Real timers plus the frame harness, for tests that assert a value produced
// inside requestAnimationFrame. Installed after the browser stubs so it is
// restored before them.
const useFrameClock = (): DeterministicAnimationFrames => {
  vi.useRealTimers();
  frames = installDeterministicAnimationFrames();
  return frames;
};

describe("#1871 SessionRowMenu surface mechanics", () => {
  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    vi.useFakeTimers();
  });

  afterEach(() => {
    host?.dispose();
    host = null;
    frames?.restore();
    frames = null;
    vi.useRealTimers();
    setViewport(originalInnerWidth, originalInnerHeight);
    cleanupDom?.();
    cleanupDom = null;
    document.body.replaceChildren();
    vi.restoreAllMocks();
    delete rects.menu;
    delete rects.anchor;
    delete rects.flyout;
  });

  // 1.
  it("open={false} renders no menu anywhere in the document", () => {
    mountHost();
    expect(document.querySelector(".session-context-menu")).toBeNull();
    expect(q("rootAgent.menu")).toBeNull();
  });

  // 2.
  it("opening registers the three window listeners; click, contextmenu and Escape each dismiss once, a plain key does not", async () => {
    const h = mountHost();
    await openMenu();
    windowKey("a");
    expect(h.onDismiss).toHaveBeenCalledTimes(0);
    expect(q("rootAgent.menu")).not.toBeNull();
    windowEvent("click");
    expect(h.onDismiss).toHaveBeenCalledTimes(1);
    expect(q("rootAgent.menu")).toBeNull();
    await openMenu();
    windowEvent("contextmenu");
    expect(h.onDismiss).toHaveBeenCalledTimes(2);
    await openMenu();
    windowKey("Escape");
    expect(h.onDismiss).toHaveBeenCalledTimes(3);
    expect(q("rootAgent.menu")).toBeNull();
  });

  // 3.
  it("unmounting while open removes all three listeners, and they are proved present first", async () => {
    const h = mountHost();
    await openMenu();
    windowEvent("click");
    expect(h.onDismiss).toHaveBeenCalledTimes(1);
    await openMenu();
    expect(q("rootAgent.menu")).not.toBeNull();
    h.dispose();
    host = null;
    windowEvent("click");
    windowEvent("contextmenu");
    windowKey("Escape");
    expect(h.onDismiss).toHaveBeenCalledTimes(1);
  });

  // 4. Clamping, six cases: three arms on each axis.
  it.each([
    { name: "a: x, oversized collapse", viewport: [200, 1000], menu: [300, 50], request: [150, 10], expect: ["left", "8px"] },
    { name: "b: x, the MARGIN floor", viewport: [1000, 1000], menu: [220, 50], request: [-50, 10], expect: ["left", "8px"] },
    { name: "c: x, innerWidth - width - MARGIN", viewport: [1000, 1000], menu: [220, 50], request: [900, 10], expect: ["left", "772px"] },
    { name: "d: y, oversized collapse", viewport: [1000, 200], menu: [50, 300], request: [10, 150], expect: ["top", "8px"] },
    { name: "e: y, the MARGIN floor", viewport: [1000, 1000], menu: [50, 220], request: [10, -50], expect: ["top", "8px"] },
    { name: "f: y, innerHeight - height - MARGIN", viewport: [1000, 1000], menu: [50, 220], request: [10, 900], expect: ["top", "772px"] },
  ])("clamps $name", async (row) => {
    setViewport(row.viewport[0], row.viewport[1]);
    rects.menu = { width: row.menu[0], height: row.menu[1] };
    stubRects();
    mountHost();
    await openMenu({ x: row.request[0], y: row.request[1] });
    const style = menuEl().style;
    expect(style[row.expect[0] as "left" | "top"]).toBe(row.expect[1]);
  });

  // 5. Dynamic height re-clamps, on the frame clock.
  it("re-clamps when the telegram bot list grows the menu past the bottom edge", async () => {
    const clock = useFrameClock();
    setViewport(1000, 1000);
    rects.menu = { width: 200, height: 100 };
    stubRects();
    const h = mountHost();
    await openMenu({ x: 10, y: 850 });
    expect(menuEl().style.top).toBe("850px");
    rects.menu = { width: 200, height: 300 };
    h.setCaps({
      telegram: {
        on: false,
        onSelect: noop,
        bridgeColor: null,
        bots: [
          { id: "b1", label: "One", token: "t", chatId: 1, color: "#111" },
          { id: "b2", label: "Two", token: "t", chatId: 2, color: "#222" },
        ],
        onSelectBot: noop,
      },
    });
    expect(q("rootAgent.menu.telegram.bot.b1")).not.toBeNull();
    expect(menuEl().style.top).toBe("850px");
    await clock.flush();
    expect(menuEl().style.top).toBe("692px");
  });

  // 6. Teardown cancels both timers and clears the flyout slot (CHANGE 1).
  describe("teardown", () => {
    it("part 1: an unmount before the registration timer fires leaves no listener behind", () => {
      const h = mountHost();
      openMenuWithoutRegistering();
      h.dispose();
      host = null;
      vi.runAllTimers();
      windowEvent("click");
      expect(h.onDismiss).toHaveBeenCalledTimes(0);
    });

    it("part 2: a dismiss before the timer fires cancels it, so a reopen registers exactly once", async () => {
      mountHost();
      const addListener = vi.spyOn(window, "addEventListener");
      openMenuWithoutRegistering();
      // Dismiss through an item, before advancing any timer.
      click(q("rootAgent.restart")!);
      expect(q("rootAgent.menu")).toBeNull();
      await openMenu();
      expect(q("rootAgent.menu")).not.toBeNull();
      expect(addListener).toHaveBeenCalledTimes(3);
    });

    it("part 3: a pending flyout close does not survive the dismiss to kill a fresh flyout", async () => {
      mountHost();
      await openMenu({ caps: { restart: { onSelect: noop }, repos: reposWithBrowse() } });
      mouseEnter(q("rootAgent.menu.repo.0")!);
      expect(flyouts()).toHaveLength(1);
      mouseLeave(q("rootAgent.menu.repo.0")!);
      vi.advanceTimersByTime(100);
      expect(flyouts()).toHaveLength(1);
      windowEvent("click");
      expect(q("rootAgent.menu")).toBeNull();
      await openMenu({ caps: { restart: { onSelect: noop }, repos: reposWithBrowse() } });
      mouseEnter(q("rootAgent.menu.repo.0")!);
      expect(flyouts()).toHaveLength(1);
      expect(() => vi.advanceTimersByTime(100)).not.toThrow();
      expect(flyouts()).toHaveLength(1);
    });

    it("part 4: the flyout slot is reset on dismiss, with no timer involved", async () => {
      mountHost();
      await openMenu({ caps: { restart: { onSelect: noop }, repos: reposWithBrowse() } });
      mouseEnter(q("rootAgent.menu.repo.0")!);
      expect(flyouts()).toHaveLength(1);
      windowEvent("click");
      expect(q("rootAgent.menu")).toBeNull();
      await openMenu({ caps: { restart: { onSelect: noop }, repos: reposWithBrowse() } });
      expect(q("rootAgent.menu")).not.toBeNull();
      expect(flyouts()).toHaveLength(0);
    });
  });

  // 7. reclamp() is driven by the catalogue, not by the host.
  describe("reclamp on the task-title editor", () => {
    const editor = (editing: boolean): SessionRowMenuCaps => ({
      editTaskTitle: {
        editing,
        draft: "Title",
        busy: false,
        error: null,
        onDraft: noop,
        onStart: noop,
        onSave: noop,
        onCancel: noop,
      },
    });

    it("through requestAnimationFrame", async () => {
      const clock = useFrameClock();
      setViewport(1000, 1000);
      rects.menu = { width: 200, height: 100 };
      stubRects();
      const h = mountHost();
      await openMenu({ x: 10, y: 850, caps: editor(false) });
      expect(menuEl().style.top).toBe("850px");
      rects.menu = { width: 200, height: 300 };
      h.setCaps(editor(true));
      expect(menuEl().querySelector(".session-context-title-edit")).not.toBeNull();
      await clock.flush();
      expect(menuEl().style.top).toBe("692px");
    });

    it("through the setTimeout(0) fallback when requestAnimationFrame is absent", async () => {
      useFrameClock();
      setViewport(1000, 1000);
      rects.menu = { width: 200, height: 100 };
      stubRects();
      const h = mountHost();
      await openMenu({ x: 10, y: 850, caps: editor(false) });
      expect(menuEl().style.top).toBe("850px");
      Reflect.deleteProperty(globalThis, "requestAnimationFrame");
      expect(typeof window.requestAnimationFrame).toBe("undefined");
      rects.menu = { width: 200, height: 300 };
      h.setCaps(editor(true));
      expect(menuEl().style.top).toBe("850px");
      await flushRegistration();
      expect(menuEl().style.top).toBe("692px");
    });
  });

  // 8. Flyout timing at the 180 ms boundary.
  it("keeps a hovered flyout open for 179 ms after mouseleave, closes it at 180, and re-entry cancels the close", async () => {
    mountHost();
    await openMenu({ caps: { repos: reposWithBrowse() } });
    const trigger = q("rootAgent.menu.repo.0")!;
    mouseEnter(trigger);
    expect(flyouts()).toHaveLength(1);
    mouseLeave(trigger);
    vi.advanceTimersByTime(179);
    expect(flyouts()).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(flyouts()).toHaveLength(0);

    mouseEnter(trigger);
    expect(flyouts()).toHaveLength(1);
    mouseLeave(trigger);
    vi.advanceTimersByTime(100);
    mouseEnter(trigger);
    vi.advanceTimersByTime(200);
    expect(flyouts()).toHaveLength(1);
  });

  // 9. Flyout positioning, on the frame clock.
  describe("flyout positioning", () => {
    it("uses the 220 x 88 fallback before layout, clamped inside the margin", async () => {
      useFrameClock();
      setViewport(300, 1000);
      rects.anchor = { left: 100, top: 50, width: 100, height: 20 };
      rects.flyout = { width: 150, height: 40 };
      stubRects();
      mountHost();
      await openMenu({ caps: { repos: reposWithBrowse() } });
      mouseEnter(q("rootAgent.menu.repo.0")!);
      const panel = flyouts()[0];
      // x = 204; 204 + 220 + 8 > 300 -> flip to 100 - 220 - 4 = -124 -> floor 8.
      expect(panel.style.left).toBe("8px");
      expect(panel.style.top).toBe("50px");
    });

    it("with room on the right, the measured flyout sits at anchorRect.right + 4", async () => {
      const clock = useFrameClock();
      setViewport(1000, 1000);
      rects.anchor = { left: 100, top: 50, width: 100, height: 20 };
      rects.flyout = { width: 150, height: 40 };
      stubRects();
      mountHost();
      await openMenu({ caps: { repos: reposWithBrowse() } });
      mouseEnter(q("rootAgent.menu.repo.0")!);
      await clock.flush();
      const panel = flyouts()[0];
      expect(panel.style.left).toBe("204px");
      expect(panel.style.top).toBe("50px");
    });

    it("with no room on the right, the measured flyout flips to anchorRect.left - width - 4", async () => {
      const clock = useFrameClock();
      setViewport(500, 1000);
      rects.anchor = { left: 300, top: 50, width: 100, height: 20 };
      rects.flyout = { width: 150, height: 40 };
      stubRects();
      mountHost();
      await openMenu({ caps: { repos: reposWithBrowse() } });
      mouseEnter(q("rootAgent.menu.repo.0")!);
      const panel = flyouts()[0];
      // Fallback first: 404 + 220 + 8 > 500 -> 300 - 220 - 4 = 76.
      expect(panel.style.left).toBe("76px");
      await clock.flush();
      // Measured: 404 + 150 + 8 > 500 -> 300 - 150 - 4 = 146.
      expect(panel.style.left).toBe("146px");
    });
  });

  // 10. Flyout mutual exclusion, and the timer that can still defeat it.
  describe("flyout mutual exclusion", () => {
    const bothCaps = (): SessionRowMenuCaps => ({ repos: reposWithBrowse(), addToGroup: groupSpec() });

    it("opening Add to Group while the repo flyout is open leaves exactly one flyout, the right one", async () => {
      mountHost();
      await openMenu({ caps: bothCaps() });
      mouseEnter(q("rootAgent.menu.repo.0")!);
      expect(flyouts()).toHaveLength(1);
      expect(flyouts()[0].getAttribute("data-ac-testid")).toBe("rootAgent.menu.repo.0.browse.flyout");
      click(q(GROUP_TEST_IDS.trigger)!);
      expect(flyouts()).toHaveLength(1);
      expect(flyouts()[0].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.flyout);
    });

    it("opening the repo flyout while Add to Group is open leaves exactly one flyout, the right one", async () => {
      mountHost();
      await openMenu({ caps: bothCaps() });
      click(q(GROUP_TEST_IDS.trigger)!);
      expect(flyouts()).toHaveLength(1);
      expect(flyouts()[0].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.flyout);
      mouseEnter(q("rootAgent.menu.repo.0")!);
      expect(flyouts()).toHaveLength(1);
      expect(flyouts()[0].getAttribute("data-ac-testid")).toBe("rootAgent.menu.repo.0.browse.flyout");
    });

    it("opening Add to Group cancels the repo flyout's pending close", async () => {
      mountHost();
      await openMenu({ caps: bothCaps() });
      const trigger = q("rootAgent.menu.repo.0")!;
      mouseEnter(trigger);
      mouseLeave(trigger);
      click(q(GROUP_TEST_IDS.trigger)!);
      expect(flyouts()[0].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.flyout);
      vi.advanceTimersByTime(200);
      expect(flyouts()).toHaveLength(1);
      expect(flyouts()[0].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.flyout);
    });
  });

  // 11. repos click semantics (section 7.5 rule 1), not only hover semantics.
  it("a repo trigger opens the folder on click, the flyout on ArrowRight, and Escape closes only what is open", async () => {
    const onOpenRepo = vi.fn();
    const h = mountHost();
    await openMenu({ caps: { repos: reposWithBrowse({ onOpenRepo }) } });
    const trigger = q("rootAgent.menu.repo.0")!;
    expect(trigger.getAttribute("title")).toBe(REPO_A.sourcePath);

    click(trigger);
    expect(onOpenRepo).toHaveBeenCalledTimes(1);
    expect(onOpenRepo).toHaveBeenCalledWith(REPO_A.sourcePath);
    expect(flyouts()).toHaveLength(0);
    // The trigger click dismissed through the catalogue (dismiss, then select).
    expect(h.onDismiss).toHaveBeenCalledTimes(1);

    await openMenu({ caps: { repos: reposWithBrowse({ onOpenRepo }) } });
    const trigger2 = q("rootAgent.menu.repo.0")!;
    keyDown(trigger2, "ArrowRight");
    expect(flyouts()).toHaveLength(1);
    await Promise.resolve();
    const firstItem = q("rootAgent.menu.repo.0.browse.main")!;
    expect(document.activeElement).toBe(firstItem);

    keyDown(firstItem, "Escape");
    expect(flyouts()).toHaveLength(0);
    expect(q("rootAgent.menu")).not.toBeNull();
    expect(h.onDismiss).toHaveBeenCalledTimes(1);

    keyDown(trigger2, "Escape");
    expect(h.onDismiss).toHaveBeenCalledTimes(2);
    expect(q("rootAgent.menu")).toBeNull();
  });

  // 12. addToGroup click semantics (rule 2).
  it.each(["click", "Enter", " "])("Add to Group opens its flyout on %s", async (how) => {
    mountHost();
    await openMenu({ caps: { addToGroup: groupSpec() } });
    const trigger = q(GROUP_TEST_IDS.trigger)!;
    expect(flyouts()).toHaveLength(0);
    if (how === "click") click(trigger);
    else keyDown(trigger, how);
    expect(flyouts()).toHaveLength(1);
    expect(flyouts()[0].getAttribute("data-ac-testid")).toBe(GROUP_TEST_IDS.flyout);
    expect(q("rootAgent.menu")).not.toBeNull();
  });
});
