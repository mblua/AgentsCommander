// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Show, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { PreflightTestPanel } from "./PreflightTestPanel";
import { FakeTransport } from "../../../shared/testing/fake-transport";
import {
  baseSettings, discovery, installBrowserDomStubs, renderWithFakeTransport,
  resetUiStoresForTests,
} from "../../../shared/testing/ui-harness";
import { projectStore } from "../../stores/project";
import { PreflightTestAPI, __setTransportForTests } from "../../../shared/ipc";
import type {
  AppSettings, PreflightStageReport, PreflightTargetDescription, PreflightTestSnapshot,
} from "../../../shared/types";

// #3516 - the shared test panel: chained polling, cancel-on-close, stale and
// cancelled results, and Escape ownership. Fake timers: each poll is scheduled
// 500 ms after an awaited response, so every poll is one advanceTimersByTimeAsync.

const PROJECT = "C:\\Project";
const ROOM = `${PROJECT}\\.ac\\room-1-dev`;
const TARGET = `${ROOM}\\__agent_dev`;
const SAVED: AppSettings = baseSettings({
  agents: [{
    id: "codex", label: "Codex", command: "codex", color: "#000", envs: [],
    isolatedHome: false, preflight: "check",
  }],
});

function stage(over: Partial<PreflightStageReport> = {}): PreflightStageReport {
  return {
    source: "codingAgent", label: "Coding Agent", effectiveProfile: "A", commandText: "check",
    timeoutSeconds: 60, status: "running", exitCode: null, durationMs: null, stdout: "",
    stderr: "", stdoutTruncated: false, stderrTruncated: false, detail: null,
    cleanupDefect: null, ...over,
  };
}

function snap(over: Partial<PreflightTestSnapshot> = {}): PreflightTestSnapshot {
  return {
    runId: "run-1", agentId: "codex", replicaPath: TARGET, effectiveProfile: "A",
    settingsRevision: "rev-1", phase: "running", outcome: null, message: null,
    stages: [stage()], ...over,
  };
}

const passed = () => snap({
  phase: "finished", outcome: "passed",
  message: "Test passed. Preflights will run again when you launch.",
  stages: [stage({ status: "passed", exitCode: 0, durationMs: 12, stdout: "ok" })],
});

function target(over: Partial<PreflightTargetDescription> = {}): PreflightTargetDescription {
  return {
    agentId: "codex", replicaPath: TARGET, cwd: TARGET, requestedProfile: "A",
    effectiveProfile: "A", fallbackApplied: false, stopped: true,
    stages: [{ source: "codingAgent", label: "Coding Agent", commandText: "check",
      timeoutSeconds: 60 }],
    settingsRevision: "rev-1", activeRunId: null, unavailableReason: null, ...over,
  };
}

type Reply<T> = T | Error | Promise<T>;

interface Harness {
  fake: FakeTransport;
  root: HTMLElement;
  gets: Reply<PreflightTestSnapshot>[];
  maxInFlight: () => number;
  setDraft: (edit: (draft: AppSettings) => void) => void;
  setPending: (pending: boolean) => void;
  closed: () => number;
  cleanup: () => void;
}

async function settle(ms = 0) {
  await vi.advanceTimersByTimeAsync(ms);
}

async function mount(describeReply: () => Reply<PreflightTargetDescription>): Promise<Harness> {
  const fake = new FakeTransport();
  const gets: Reply<PreflightTestSnapshot>[] = [];
  let inFlight = 0;
  let max = 0;
  fake.resolve("get_settings", SAVED);
  fake.onInvoke("open_project", () => ({ path: PROJECT, ...rooms() }));
  fake.onInvoke("discover_project", () => rooms());
  fake.onInvoke("describe_preflight_target", () => unwrap(describeReply()));
  fake.resolve("start_preflight_test", "run-1");
  fake.onInvoke("get_preflight_test", async () => {
    inFlight++;
    max = Math.max(max, inFlight);
    try {
      return await unwrap(gets.length > 1 ? gets.shift()! : gets[0]);
    } finally {
      inFlight--;
    }
  });
  fake.onInvoke("cancel_preflight_test", () => snap({ message: "Cancelling…" }));
  const [draft, setDraft] = createStore<AppSettings>(structuredClone(SAVED));
  const [pending, setPending] = createSignal(false);
  const [open, setOpen] = createSignal(true);
  let closed = 0;
  const r = renderWithFakeTransport(() => (
    <Show when={open()}>
      <PreflightTestPanel
        agentId="codex"
        letter={null}
        draft={draft}
        timeoutDraftPending={pending()}
        onClose={() => { closed++; setOpen(false); }}
      />
    </Show>
  ), fake);
  await projectStore.loadProject(PROJECT);
  await settle();
  return {
    fake, root: r.root, gets, maxInFlight: () => max,
    setDraft: (edit) => setDraft((d) => {
      const next = structuredClone(d);
      edit(next);
      return next;
    }),
    setPending, closed: () => closed, cleanup: r.cleanup,
  };
}

function unwrap<T>(reply: Reply<T>): Promise<T> {
  if (reply instanceof Error) return Promise.reject(reply.message);
  return Promise.resolve(reply);
}

function rooms() {
  return discovery({
    workgroups: [{
      name: "room-1-dev", path: ROOM, task: null,
      agents: [{ name: "dev", path: TARGET, repoPaths: [], isCoordinator: false }],
    }],
  });
}

function q<T extends HTMLElement = HTMLElement>(root: HTMLElement, id: string): T | null {
  return root.querySelector<T>(`[data-ac-testid="settings.preflightTest${id}"]`);
}
const status = (h: Harness) => q(h.root, ".status")?.textContent ?? "";
const runButton = (h: Harness) => q<HTMLButtonElement>(h.root, ".run");

async function select(h: Harness) {
  const el = q<HTMLSelectElement>(h.root, ".target")!;
  el.value = TARGET;
  el.dispatchEvent(new Event("change", { bubbles: true }));
  await settle();
}

async function startRun(h: Harness) {
  await select(h);
  runButton(h)!.click();
  await settle();
}

describe("PreflightTestAPI (#3516)", () => {
  it("sends the exact command names and argument keys", async () => {
    const fake = new FakeTransport();
    fake.resolve("describe_preflight_target", target());
    fake.resolve("start_preflight_test", "run-1");
    fake.resolve("get_preflight_test", snap());
    fake.resolve("cancel_preflight_test", snap());
    const restore = __setTransportForTests(fake);
    try {
      await PreflightTestAPI.describe("codex", "B", TARGET);
      await PreflightTestAPI.start("codex", null, TARGET, "rev-1");
      await PreflightTestAPI.get("run-1");
      await PreflightTestAPI.cancel("run-1");
      expect(fake.calls.map((c) => [c.cmd, c.args])).toEqual([
        ["describe_preflight_target",
          { agentId: "codex", profileLetter: "B", replicaPath: TARGET }],
        ["start_preflight_test",
          { agentId: "codex", profileLetter: null, replicaPath: TARGET,
            settingsRevision: "rev-1" }],
        ["get_preflight_test", { runId: "run-1" }],
        ["cancel_preflight_test", { runId: "run-1" }],
      ]);
    } finally {
      restore();
    }
  });
});

describe("PreflightTestPanel (#3516)", () => {
  let cleanupDom: (() => void) | null = null;
  let h: Harness | null = null;

  beforeEach(() => {
    vi.useFakeTimers();
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    h?.cleanup();
    h = null;
    vi.useRealTimers();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("shows the saved chain, the notes and progress, then the result", async () => {
    h = await mount(() => target());
    expect(status(h)).toBe("Which stopped agent should run this test?");
    await select(h);
    const info = q(h.root, ".info")!.textContent!;
    expect(info).toContain(`Folder: ${TARGET}`);
    expect(info).toContain("Coding Agent: check");
    expect(info).toContain("Saved timeout: 60 seconds");
    expect(info).toContain("The test does not copy the agent configuration first.");
    h.gets.push(snap(), passed());
    runButton(h)!.click();
    await settle();
    expect(status(h)).toBe("Running Coding Agent preflight…");
    expect(q(h.root, ".cancel")).toBeTruthy();
    await settle(500);
    expect(q(h.root, ".result")!.textContent).toContain("Test passed.");
    expect(q(h.root, ".run")).toBeTruthy();
    q<HTMLButtonElement>(h.root, ".stage.codingAgent.output")!.click();
    expect(q(h.root, ".stage.codingAgent")!.textContent).toContain("Duration: 12 ms");
    const gets = h.fake.callsFor("get_preflight_test").length;
    await settle(2000);
    expect(h.fake.callsFor("get_preflight_test")).toHaveLength(gets);
  });

  it("shows Cancelling… then the cancelled result, never a pass", async () => {
    h = await mount(() => target());
    h.gets.push(snap());
    await startRun(h);
    q<HTMLButtonElement>(h.root, ".cancel")!.click();
    await settle();
    expect(status(h)).toBe("Cancelling…");
    h.gets.length = 0;
    h.gets.push(snap({
      phase: "finished", outcome: "cancelled", message: "Test cancelled. No passing result.",
      stages: [stage({ status: "cancelled" })],
    }));
    await settle(500);
    const result = q(h.root, ".result")!;
    expect(result.textContent).toContain("Test cancelled. No passing result.");
    expect(h.root.textContent).not.toContain("Test passed");
    expect(result.classList.contains("passed")).toBe(false);
  });

  it.each([
    ["an unsaved draft", (x: Harness) => x.setDraft((d) => { d.agents[0].preflight = "new"; }),
      () => target(), "Save your changes before testing preflights."],
    ["a pending timeout draft", (x: Harness) => x.setPending(true), () => target(),
      "Save your changes before testing preflights."],
    ["a running target", () => {}, () => target({ stopped: false }),
      "Stop this agent before testing its preflights."],
    ["an empty chain", () => {}, () => target({ stages: [] }),
      "No preflight commands configured. Nothing to test."],
  ])("disables Run test for %s", async (_name, arrange, reply, text) => {
    h = await mount(reply);
    arrange(h);
    await select(h);
    expect(status(h)).toBe(text);
    expect(runButton(h)!.disabled).toBe(true);
  });

  it("marks an edited result Stale and keeps it Stale after a revert", async () => {
    h = await mount(() => target());
    h.gets.push(passed());
    await startRun(h);
    expect(q(h.root, ".result")!.dataset.acState).toBe("passed");
    h.setDraft((d) => { d.agents[0].preflight = "edited"; });
    h.setDraft((d) => { d.agents[0].preflight = "check"; });
    expect(q(h.root, ".result")!.textContent).toContain("Previous result — settings or test");
    expect(q(h.root, ".result")!.classList.contains("stale")).toBe(true);
  });

  it.each([
    ["effective profile", { effectiveProfile: "B" }],
    ["folder", { cwd: `${TARGET}\\moved` }],
  ])("marks the result Stale when a later describe has another %s", async (_name, change) => {
    let later: Partial<PreflightTargetDescription> = {};
    h = await mount(() => target(later));
    h.gets.push(passed());
    await startRun(h);
    expect(q(h.root, ".result")!.dataset.acState).toBe("passed");
    later = change;
    await select(h);
    expect(q(h.root, ".result")!.dataset.acState).toBe("stale");
  });

  it.each([
    ["running", snap({ message: "Cancelling…" })],
    ["finished", snap({ phase: "finished", outcome: "failed", message: "Late failure." })],
  ])("ignores a delayed %s reply after finish, with one get in flight", async (_n, late) => {
    h = await mount(() => target());
    let release: (s: PreflightTestSnapshot) => void = () => {};
    h.fake.onInvoke("cancel_preflight_test", () => new Promise((r) => { release = r; }));
    h.gets.push(snap(), passed());
    await startRun(h);
    q<HTMLButtonElement>(h.root, ".cancel")!.click();
    await settle(500);
    expect(q(h.root, ".result")!.textContent).toContain("Test passed.");
    release(late);
    await settle();
    expect(q(h.root, ".result")!.textContent).toContain("Test passed.");
    await settle(2000);
    expect(q(h.root, ".result")).toBeTruthy();
    expect(q(h.root, ".cancel")).toBeNull();
    expect(h.fake.callsFor("get_preflight_test")).toHaveLength(2);
    expect(h.maxInFlight()).toBe(1);
  });

  it.each(["unmount", "close"])("cancels an unfinished run once on %s", async (how) => {
    h = await mount(() => target());
    h.gets.push(snap());
    await startRun(h);
    const x = h;
    if (how === "close") q<HTMLButtonElement>(x.root, ".close")!.click();
    else { x.cleanup(); h = null; }
    await settle(2000);
    expect(x.fake.callsFor("cancel_preflight_test")).toEqual([
      { cmd: "cancel_preflight_test", args: { runId: "run-1" } },
    ]);
    expect(x.fake.callsFor("get_preflight_test")).toHaveLength(1);
  });

  it("closes on Escape before a document listener sees it", async () => {
    h = await mount(() => target());
    const spy = vi.fn();
    document.addEventListener("keydown", spy);
    try {
      q(h.root, ".close")!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      expect(h.closed()).toBe(1);
      expect(spy).not.toHaveBeenCalled();
    } finally {
      document.removeEventListener("keydown", spy);
    }
  });

  it("attaches to an active run from describe and polls it", async () => {
    h = await mount(() => target({ activeRunId: "run-1", stopped: false }));
    h.gets.push(snap());
    await select(h);
    expect(h.fake.lastCall("get_preflight_test")?.args).toEqual({ runId: "run-1" });
    expect(q(h.root, ".cancel")).toBeTruthy();
    expect(status(h)).toBe("Running Coding Agent preflight…");
    await settle(500);
    expect(h.fake.callsFor("get_preflight_test")).toHaveLength(2);
  });

  it("shows a rejected start and describes again", async () => {
    h = await mount(() => target());
    h.fake.reject("start_preflight_test", "Settings changed; describe again.");
    await startRun(h);
    expect(status(h)).toBe("Settings changed; describe again.");
    expect(h.fake.callsFor("describe_preflight_target")).toHaveLength(2);
  });

  it("stops on a rejected get, keeps Close, and reattaches from describe", async () => {
    let describes = 0;
    h = await mount(() => target({ activeRunId: describes++ >= 2 ? "run-2" : null }));
    h.gets.push(new Error("Unknown preflight test run."), snap({ runId: "run-2" }));
    await startRun(h);
    expect(status(h)).toBe("Unknown preflight test run.");
    expect(q(h.root, ".close")).toBeTruthy();
    expect(q(h.root, ".cancel")).toBeNull();
    await settle(2000);
    expect(h.fake.callsFor("get_preflight_test")).toHaveLength(1);
    expect(describes).toBe(2);
    await select(h);
    expect(h.fake.lastCall("get_preflight_test")?.args).toEqual({ runId: "run-2" });
    expect(q(h.root, ".cancel")).toBeTruthy();
  });
});
