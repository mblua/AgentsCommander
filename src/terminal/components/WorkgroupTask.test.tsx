// @vitest-environment jsdom
//
// #1614 section 9.1 frontend tests / section 15.4. F6
// (`WorkgroupTask.tsx:74`, `hasWorkgroupContext`) gates the TASK.md Edit and
// Clean buttons. Left unrewired it leaves both buttons permanently disabled in
// every Room, and it fails silently: nothing errors, the buttons are simply
// never clickable.
//
// The gate's case sensitivity is load-bearing and the component says so at
// :72-73: the backend is byte-exact (`session/session.rs:249` now calls
// `has_entity_prefix`), so a case-insensitive UX gate would enable buttons
// whose every click fails. Section 5.4 preserves each call site's exact case
// sensitivity, which is why F6 is a different helper from the rail's F4/F5.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import WorkgroupTask from "./WorkgroupTask";
import { terminalStore } from "../stores/terminal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";

/** The two buttons F6 gates, in render order: Edit then Clean. */
function actionButtons(): HTMLButtonElement[] {
  return Array.from(document.querySelectorAll<HTMLButtonElement>("button.workgroup-task-action"));
}

/** Bind a live session whose cwd is `cwd`, then render the component. */
async function renderWithCwd(cwd: string): Promise<{ cleanup: () => void }> {
  terminalStore.bindLockedSession(
    session({
      id: "session-1614",
      name: "agent",
      workingDirectory: cwd,
      status: "running",
    }),
    0
  );
  const fake = new FakeTransport();
  fake.resolve("task_get_title", null);
  const rendered = renderWithFakeTransport(() => <WorkgroupTask />, fake);
  await waitFor(() => expect(actionButtons().length).toBe(2));
  return rendered;
}

describe("WorkgroupTask, F6 dual-prefix gate (#1614)", () => {
  beforeEach(() => {
    resetUiStoresForTests();
    terminalStore.resetForTests();
  });

  afterEach(() => {
    resetUiStoresForTests();
    terminalStore.resetForTests();
    document.body.replaceChildren();
  });

  it("enables the Task buttons in a Room cwd", async () => {
    const rendered = await renderWithCwd("C:\\P\\.ac\\room-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(false);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("still enables the Task buttons in a legacy Workgroup cwd", async () => {
    // Rule P2: the legacy case is kept, not converted. Dual-prefix acceptance
    // is only testable while a wg-* case still exists.
    const rendered = await renderWithCwd("C:\\P\\.ac\\wg-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(false);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("stays disabled for an uppercase ROOM- directory, matching the byte-exact backend", async () => {
    const rendered = await renderWithCwd("C:\\P\\.ac\\ROOM-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(true);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("stays disabled for an uppercase WG- directory, exactly as it does today", async () => {
    const rendered = await renderWithCwd("C:\\P\\.ac\\WG-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(true);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("stays disabled outside any entity directory", async () => {
    const rendered = await renderWithCwd("C:\\P\\some\\other\\place");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(true);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("is not fooled by a directory that merely starts with the prefix letters", async () => {
    const rendered = await renderWithCwd("C:\\P\\.ac\\roomy-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(true);
      }
    } finally {
      rendered.cleanup();
    }
  });
});

describe("WorkgroupTask, frontmatter title parsing (#2608)", () => {
  beforeEach(() => {
    resetUiStoresForTests();
    terminalStore.resetForTests();
  });

  afterEach(() => {
    resetUiStoresForTests();
    terminalStore.resetForTests();
    document.body.replaceChildren();
  });

  async function shownTitle(task: string): Promise<string | null> {
    const rendered = await renderWithCwd("C:\\P\\.ac\\room-1-t\\__agent_x");
    try {
      terminalStore.setActiveWorkgroupTask(task);
      actionButtons()[0].click();
      await waitFor(() => expect(document.querySelector(".workgroup-task-title-input")).toBeTruthy());
      return document.querySelector<HTMLInputElement>(".workgroup-task-title-input")?.value || null;
    } finally {
      rendered.cleanup();
    }
  }

  it("unescapes doubled quotes inside a single-quoted title", async () => {
    expect(await shownTitle("---\ntitle: 'it''s'\n---\nbody")).toBe("it's");
  });

  it("keeps the first title line, matched case-insensitively", async () => {
    expect(await shownTitle('---\nfoo: 1\nTITLE: "x"\ntitle: y\n---\n')).toBe("x");
  });

  it("shows no title when the frontmatter has no closer", async () => {
    expect(await shownTitle("---\ntitle: a\n")).toBeNull();
  });
});

describe("P4 authoritative description and accessible status", () => {
  const root = "C:/P/.ac/room-1-t";
  const snapshot = (overrides = {}) => ({
    workgroupRoot: root, task: "raw machine content", taskTitle: "Human title",
    description: "<b>Human description</b>\nSecond line", status: "Issue 2842\nIn progress",
    revision: "topic:4", statusRecord: null, tailIncomplete: false, ...overrides,
  });
  function publish(overrides = {}) {
    terminalStore.acceptTaskSnapshot(snapshot(overrides), terminalStore.beginTaskRead(), terminalStore.taskWriteSeq);
  }
  beforeEach(() => { terminalStore.resetForTests(); resetUiStoresForTests(); });
  afterEach(() => { vi.useRealTimers(); terminalStore.resetForTests(); resetUiStoresForTests(); document.body.replaceChildren(); });

  it("renders escaped human description and full status only in the Portal tooltip", async () => {
    const view = await renderWithCwd(root + "/__agent_x");
    try {
      expect(document.body.textContent).toContain("Loading task…");
      expect(document.querySelector(".workgroup-task-title")).toBeNull();
      publish();
      const title = document.querySelector<HTMLElement>(".workgroup-task-title")!;
      expect(title.textContent).toBe("Human title");
      expect(title.tabIndex).toBe(0);
      expect(document.querySelector(".workgroup-task-text")?.textContent).toBe(snapshot().description);
      expect(document.querySelector(".workgroup-task-text b")).toBeNull();
      const tooltip = document.querySelector<HTMLElement>('[role="tooltip"]')!;
      expect(tooltip.textContent).toBe(snapshot().status);
      expect(title.getAttribute("aria-describedby")).toBe(tooltip.id);
      expect(document.querySelector(".workgroup-task-panel")?.contains(tooltip)).toBe(false);
      title.focus();
      expect(tooltip.style.display).toBe("block");
      title.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      expect(tooltip.style.display).toBe("none");
      window.dispatchEvent(new Event("resize"));
      publish({ status: "Refreshed status" });
      expect(document.querySelector<HTMLElement>('[role="tooltip"]')!.style.display).toBe("none");
      title.blur(); title.focus();
      expect(document.querySelector<HTMLElement>('[role="tooltip"]')!.style.display).toBe("block");
    } finally { view.cleanup(); }
  });

  it("retains human text on refresh/error while withholding stale tooltip", async () => {
    const view = await renderWithCwd(root + "/__agent_x");
    try {
      publish();
      terminalStore.invalidateTask(root);
      expect(document.body.textContent).toContain("Refreshing task…");
      expect(document.querySelector('[role="tooltip"]')).toBeNull();
      expect(document.querySelector(".workgroup-task-text")?.textContent).toBe(snapshot().description);
      terminalStore.failTaskRead(terminalStore.beginTaskRead());
      expect(document.body.textContent).toContain("Could not refresh the task.");
      expect(document.querySelector('[role="tooltip"]')).toBeNull();
      publish({ taskTitle: null, status: null, revision: "legacy:0", tailIncomplete: true });
      expect(document.querySelector(".workgroup-task-title")?.textContent).toBe("--No title specified--");
      expect(document.querySelector(".workgroup-task-title")?.hasAttribute("aria-describedby")).toBe(false);
      expect(document.body.textContent).toContain("Task history is incomplete.");
    } finally { view.cleanup(); }
  });

  it("shows initial read error separately from a legacy null success", async () => {
    const view = await renderWithCwd(root + "/__agent_x");
    try {
      terminalStore.failTaskRead(terminalStore.beginTaskRead());
      expect(document.body.textContent).toContain("Could not read the task.");
      publish({ task: null, taskTitle: null, description: "", status: null });
      expect(document.querySelector(".workgroup-task-title")?.textContent).toBe("--No title specified--");
      expect(document.body.textContent).not.toContain("Could not read");
    } finally { view.cleanup(); }
  });

  it("honors 150 ms pointer gap, tooltip entry, scroll keys and teardown", async () => {
    const view = await renderWithCwd(root + "/__agent_x");
    try {
      publish(); vi.useFakeTimers();
      const title = document.querySelector<HTMLElement>(".workgroup-task-title")!;
      const tooltip = document.querySelector<HTMLElement>('[role="tooltip"]')!;
      title.dispatchEvent(new Event("pointerenter"));
      title.dispatchEvent(new Event("pointerleave"));
      vi.advanceTimersByTime(149);
      expect(tooltip.style.display).toBe("block");
      tooltip.dispatchEvent(new Event("pointerenter"));
      vi.advanceTimersByTime(200);
      expect(tooltip.style.display).toBe("block");
      Object.defineProperties(tooltip, { scrollHeight: { value: 500 }, clientHeight: { value: 100 } });
      for (const [key, expected] of [["ArrowDown", 32], ["PageDown", 132], ["Home", 0], ["End", 500]] as const) {
        const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
        title.dispatchEvent(event); expect(event.defaultPrevented).toBe(true); expect(tooltip.scrollTop).toBe(expected);
      }
      const tab = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
      title.dispatchEvent(tab); expect(tab.defaultPrevented).toBe(false);
      tooltip.dispatchEvent(new Event("pointerleave")); vi.advanceTimersByTime(150);
      expect(tooltip.style.display).toBe("none");
      view.cleanup(); vi.runAllTimers(); expect(document.querySelector('[role="tooltip"]')).toBeNull();
    } finally { view.cleanup(); }
  });
});
