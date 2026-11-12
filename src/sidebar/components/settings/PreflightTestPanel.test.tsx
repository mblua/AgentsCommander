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
  AppSettings, PreflightStageReport, PreflightTargetDescription, PreflightTestDraft,
  PreflightTestSnapshot, PreflightTestTarget,
} from "../../../shared/types";

// #3516/#3832 - the shared test panel: draft payload, target picker, chained
// polling, cancel-on-close, stale and cancelled results, and Escape ownership.
// Fake timers: each poll is scheduled 500 ms after an awaited response, so every
// poll is one advanceTimersByTimeAsync.

const PROJECT = "C:\\Project";
const ROOM = `${PROJECT}\\.ac\\room-1-dev`;
const TARGET = `${ROOM}\\__agent_dev`;
const OTHER = `${ROOM}\\__agent_qa`;
const SAVED: AppSettings = baseSettings({
  agents: [{
    id: "codex", label: "Codex", command: "codex", color: "#000", envs: [],
    isolatedHome: false, preflight: "check",
  }],
});
SAVED.codingAgentProfiles.profilesByAgent.codex = {
  A: { enabled: true, command: "codex", env: {}, notes: "", preflight: "saved-a",
    preflightTimeoutSeconds: 30 },
};
// Commands the mount may send; anything else (save, update, session) fails the test.
const ALLOWED = new Set([
  "get_settings", "open_project", "discover_project", "list_preflight_test_targets",
  "describe_preflight_target", "start_preflight_test", "get_preflight_test",
  "cancel_preflight_test", "task_get_snapshot_at",
]);

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

function row(path: string, over: Partial<PreflightTestTarget> = {}): PreflightTestTarget {
  return {
    replicaPath: path, requestedProfile: "A", effectiveProfile: "A", fallbackApplied: false,
    running: false, ...over,
  };
}

type Reply<T> = T | Error | Promise<T>;
type Args = Record<string, unknown>;

interface MountOptions {
  describe?: (args: Args) => Reply<PreflightTargetDescription>;
  list?: () => Reply<PreflightTestTarget[]>;
  saved?: () => Reply<AppSettings>;
  letter?: string | null;
}

interface Harness {
  fake: FakeTransport;
  root: HTMLElement;
  gets: Reply<PreflightTestSnapshot>[];
  draft: AppSettings;
  maxInFlight: () => number;
  setDraft: (edit: (draft: AppSettings) => void) => void;
  setPending: (pending: boolean) => void;
  closed: () => number;
  unmount: () => void;
  cleanup: () => void;
}

async function settle(ms = 0) {
  await vi.advanceTimersByTimeAsync(ms);
}

async function mount(opts: MountOptions = {}): Promise<Harness> {
  const fake = new FakeTransport();
  const gets: Reply<PreflightTestSnapshot>[] = [];
  let inFlight = 0;
  let max = 0;
  fake.onInvoke("get_settings", () => unwrap(opts.saved ? opts.saved() : SAVED));
  fake.onInvoke("open_project", () => ({ path: PROJECT, ...rooms() }));
  fake.onInvoke("discover_project", () => rooms());
  fake.onInvoke("list_preflight_test_targets",
    () => unwrap(opts.list ? opts.list() : [row(TARGET)]));
  fake.onInvoke("describe_preflight_target",
    (args) => unwrap(opts.describe ? opts.describe(args) : target()));
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
        letter={opts.letter ?? null}
        draft={draft}
        timeoutDraftPending={pending()}
        onClose={() => { closed++; setOpen(false); }}
      />
    </Show>
  ), fake);
  await projectStore.loadProject(PROJECT);
  await settle();
  return {
    fake, root: r.root, gets, draft, maxInFlight: () => max,
    setDraft: (edit) => setDraft((d) => {
      const next = structuredClone(d);
      edit(next);
      return next;
    }),
    setPending, closed: () => closed, unmount: () => setOpen(false), cleanup: r.cleanup,
  };
}

function unwrap<T>(reply: Reply<T>): Promise<T> {
  if (reply instanceof Error) return Promise.reject(reply.message);
  return Promise.resolve(reply);
}

function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

function rooms() {
  return discovery({
    workgroups: [{
      name: "room-1-dev", path: ROOM, task: null,
      agents: [
        { name: "dev", path: TARGET, repoPaths: [], isCoordinator: false },
        { name: "qa", path: OTHER, repoPaths: [], isCoordinator: false },
      ],
    }],
  });
}

function q<T extends HTMLElement = HTMLElement>(root: HTMLElement, id: string): T | null {
  return root.querySelector<T>(`[data-ac-testid="settings.preflightTest${id}"]`);
}
const status = (h: Harness) => q(h.root, ".status")?.textContent ?? "";
const runButton = (h: Harness) => q<HTMLButtonElement>(h.root, ".run");
const input = (h: Harness) => q<HTMLInputElement>(h.root, ".target")!;
const list = (h: Harness) => q(h.root, ".target.list")!;
const options = (h: Harness) => [...list(h).querySelectorAll<HTMLElement>("[role=option]")];
const optionFor = (h: Harness, path: string) =>
  options(h).find((o) => o.dataset.acDetail === path)!;
const escape = (over: KeyboardEventInit = {}) =>
  new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true, ...over });

function search(h: Harness, text: string) {
  input(h).value = text;
  input(h).dispatchEvent(new InputEvent("input", { bubbles: true }));
}

async function select(h: Harness, path = TARGET) {
  input(h).focus();
  optionFor(h, path).click();
  await settle();
}

async function startRun(h: Harness) {
  await select(h);
  runButton(h)!.click();
  await settle();
}

function draftOf(args: Args | undefined): PreflightTestDraft {
  return args?.draft as PreflightTestDraft;
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

describe("PreflightTestPanel (#3516/#3832)", () => {
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

  it("shows the draft chain, the notes and progress, then the result", async () => {
    h = await mount();
    expect(status(h)).toBe("Which stopped agent should run this test?");
    const lists = h.fake.callsFor("list_preflight_test_targets");
    expect(lists[lists.length - 1]?.args).toEqual({
      agentId: "codex", replicaPaths: [TARGET, OTHER] });
    await select(h);
    const info = q(h.root, ".info")!.textContent!;
    expect(info).toContain(`Folder: ${TARGET}`);
    expect(info).toContain("Coding Agent: check");
    expect(info).toContain("Timeout: 60 seconds");
    expect(info).toContain("Runs both preflights in launch order, as shown here.");
    expect(info).toContain("Only the preflight commands and timeouts use your unsaved changes.");
    expect(info).toContain("The test does not copy the agent configuration first.");
    expect(info).not.toContain("saved preflights");
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
    h = await mount();
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
    ["an invalid timeout draft", (x: Harness) => x.setPending(true), () => target(),
      "Enter a whole number of seconds from 1 to 4294967295."],
    ["a running target", () => {}, () => target({ stopped: false }),
      "Stop this agent before testing its preflights."],
    ["an empty draft chain", () => {}, () => target({ stages: [] }),
      "No preflight commands configured. Nothing to test."],
  ])("disables Run test for %s", async (_name, arrange, reply, text) => {
    h = await mount({ describe: reply });
    arrange(h);
    await select(h);
    expect(status(h)).toBe(text);
    expect(runButton(h)!.disabled).toBe(true);
  });

  it("runs valid unsaved edits with the draft payload and saves nothing", async () => {
    h = await mount({
      describe: (args) => target({ stages: [{ source: "codingAgent", label: "Coding Agent",
        commandText: draftOf(args).codingAgent.command ?? "", timeoutSeconds: 9 }] }),
    });
    h.setDraft((d) => {
      d.agents[0].preflight = "draft-agent";
      d.agents[0].preflightTimeoutSeconds = 9;
      d.agents[0].command = "other-cli";
      const cell = d.codingAgentProfiles.profilesByAgent.codex.A;
      cell.preflight = "draft-a";
      cell.enabled = false;
      cell.env = { X: "1" };
    });
    await select(h);
    expect(status(h)).toBe("");
    expect(runButton(h)!.disabled).toBe(false);
    const info = q(h.root, ".info")!.textContent!;
    expect(info).toContain("Coding Agent: draft-agent");
    expect(info).toContain("Timeout: 9 seconds");
    h.gets.push(passed());
    runButton(h)!.click();
    await settle();
    const expected = {
      codingAgent: { command: "draft-agent", timeoutSeconds: 9 },
      profiles: { A: { command: "draft-a", timeoutSeconds: 30 } },
    };
    expect(h.fake.lastCall("describe_preflight_target")?.args).toEqual({
      agentId: "codex", profileLetter: null, replicaPath: TARGET, draft: expected,
    });
    expect(h.fake.lastCall("start_preflight_test")?.args).toEqual({
      agentId: "codex", profileLetter: null, replicaPath: TARGET, settingsRevision: "rev-1",
      draft: expected,
    });
    expect(q(h.root, ".result")!.textContent).toContain("Test passed.");
    expect(h.fake.calls.filter((c) => !ALLOWED.has(c.cmd))).toEqual([]);
    expect(h.draft.agents[0].preflight).toBe("draft-agent");
    expect(h.draft.codingAgentProfiles.profilesByAgent.codex.A.preflight).toBe("draft-a");
    expect(h.closed()).toBe(0);
  });

  it("uses null for undefined draft fields and sends draft text even when saved differs",
    async () => {
      h = await mount();
      h.setDraft((d) => { delete d.agents[0].preflight; });
      await select(h);
      expect(draftOf(h.fake.lastCall("describe_preflight_target")?.args)).toEqual({
        codingAgent: { command: null, timeoutSeconds: null },
        profiles: { A: { command: "saved-a", timeoutSeconds: 30 } },
      });
    });

  it("shows the unsaved badge, border and line only while the draft differs", async () => {
    h = await mount();
    const panel = q(h.root, "")!;
    expect(q(h.root, ".unsaved")).toBeNull();
    expect(q(h.root, ".unsavedLine")).toBeNull();
    expect(panel.classList.contains("preflight-test-unsaved")).toBe(false);
    h.setDraft((d) => { d.agents[0].preflight = "edited"; });
    expect(q(h.root, ".unsaved")!.textContent).toContain("Unsaved changes");
    expect(q(h.root, ".unsaved")!.nextElementSibling).toBe(q(h.root, ".close"));
    expect(q(h.root, ".unsavedLine")!.textContent)
      .toBe("Testing unsaved changes. Nothing is saved.");
    expect(panel.classList.contains("preflight-test-unsaved")).toBe(true);
    expect(h.root.textContent).not.toContain("Save your changes before testing preflights.");
    h.setDraft((d) => { d.agents[0].preflight = "check"; });
    expect(q(h.root, ".unsaved")).toBeNull();
    expect(panel.classList.contains("preflight-test-unsaved")).toBe(false);
  });

  it("suppresses the warning while saved settings load and offers Retry on failure",
    async () => {
      const saved = deferred<AppSettings>();
      let reply: () => Reply<AppSettings> = () => saved.promise;
      h = await mount({ saved: () => reply() });
      h.setDraft((d) => { d.agents[0].preflight = "edited"; });
      expect(q(h.root, ".unsaved")).toBeNull();
      saved.resolve(SAVED);
      await settle();
      expect(q(h.root, ".unsaved")).toBeTruthy();
      h.cleanup();
      resetUiStoresForTests();
      document.body.replaceChildren();
      reply = () => new Error("disk busy");
      h = await mount({ saved: () => reply() });
      expect(q(h.root, ".savedError")!.textContent).toContain("disk busy");
      reply = () => SAVED;
      q<HTMLButtonElement>(h.root, ".savedError.retry")!.click();
      await settle();
      expect(q(h.root, ".savedError")).toBeNull();
    });

  it("keeps Run disabled until the latest describe matches the draft", async () => {
    const pending: ReturnType<typeof deferred<PreflightTargetDescription>>[] = [];
    h = await mount({
      describe: (args) => {
        const d = deferred<PreflightTargetDescription>();
        pending.push(d);
        void args;
        return d.promise;
      },
    });
    await select(h);
    const first = pending[0];
    h.setDraft((d) => { d.agents[0].preflight = "newer"; });
    await settle();
    const second = pending[1];
    second.resolve(target({ stages: [{ source: "codingAgent", label: "Coding Agent",
      commandText: "newer", timeoutSeconds: 60 }] }));
    await settle();
    first.resolve(target());
    await settle();
    expect(q(h.root, ".info")!.textContent).toContain("Coding Agent: newer");
    expect(runButton(h)!.disabled).toBe(false);
    h.setDraft((d) => { d.agents[0].preflight = "newest"; });
    await settle();
    expect(q(h.root, ".info")).toBeNull();
    expect(runButton(h)!.disabled).toBe(true);
  });

  it("issues one start while start is in flight and locks the picker", async () => {
    h = await mount();
    const start = deferred<string>();
    h.fake.onInvoke("start_preflight_test", () => start.promise);
    await select(h);
    runButton(h)!.click();
    runButton(h)!.click();
    await settle();
    expect(runButton(h)!.disabled).toBe(true);
    expect(input(h).disabled).toBe(true);
    expect(h.fake.callsFor("start_preflight_test")).toHaveLength(1);
    h.gets.push(snap());
    start.resolve("run-1");
    await settle();
    expect(q(h.root, ".cancel")).toBeTruthy();
    expect(input(h).disabled).toBe(true);
  });

  it("marks an edited result Stale and keeps it Stale after a revert", async () => {
    h = await mount();
    h.gets.push(passed());
    await startRun(h);
    expect(q(h.root, ".result")!.dataset.acState).toBe("passed");
    h.setDraft((d) => { d.agents[0].preflight = "edited"; });
    h.setDraft((d) => { d.agents[0].preflight = "check"; });
    await settle();
    expect(q(h.root, ".result")!.textContent).toContain("Previous result — settings or test");
    expect(q(h.root, ".result")!.classList.contains("stale")).toBe(true);
  });

  it("keeps a run Stale when the draft is edited and reverted during the run", async () => {
    h = await mount();
    h.gets.push(snap(), passed());
    await startRun(h);
    h.setDraft((d) => { d.agents[0].preflight = "edited"; });
    await settle();
    h.setDraft((d) => { d.agents[0].preflight = "check"; });
    await settle(500);
    expect(q(h.root, ".result")!.dataset.acState).toBe("stale");
  });

  it.each([
    ["effective profile", { effectiveProfile: "B" }],
    ["folder", { cwd: `${TARGET}\\moved` }],
    ["saved revision", { settingsRevision: "rev-2" }],
  ])("marks the result Stale when a later describe has another %s", async (_name, change) => {
    let later: Partial<PreflightTargetDescription> = {};
    h = await mount({ describe: () => target(later) });
    h.gets.push(passed());
    await startRun(h);
    expect(q(h.root, ".result")!.dataset.acState).toBe("passed");
    later = change;
    await select(h);
    expect(q(h.root, ".result")!.dataset.acState).toBe("stale");
  });

  it("hides the result of another target", async () => {
    h = await mount({ list: () => [row(TARGET), row(OTHER)] });
    h.gets.push(passed());
    await startRun(h);
    expect(q(h.root, ".result")).toBeTruthy();
    expect(options(h)).toHaveLength(1);
    search(h, "");
    expect(q(h.root, ".result")).toBeNull();
    expect(options(h)).toHaveLength(2);
    await select(h, OTHER);
    expect(q(h.root, ".result")).toBeNull();
  });

  it.each([
    ["running", snap({ message: "Cancelling…" })],
    ["finished", snap({ phase: "finished", outcome: "failed", message: "Late failure." })],
  ])("ignores a delayed %s reply after finish, with one get in flight", async (_n, late) => {
    h = await mount();
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
    h = await mount();
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

  it.each(["unmount", "close"])("cancels a run whose start resolves after %s", async (how) => {
    h = await mount();
    let resolveStart: (id: string) => void = () => {};
    h.fake.onInvoke("start_preflight_test", () => new Promise((r) => { resolveStart = r; }));
    await select(h);
    runButton(h)!.click();
    await settle();
    if (how === "close") q<HTMLButtonElement>(h.root, ".close")!.click();
    else h.unmount();
    expect(q(h.root, "")).toBeNull();
    expect(h.fake.callsFor("cancel_preflight_test")).toHaveLength(0);
    resolveStart("run-late");
    await settle(2000);
    expect(h.fake.callsFor("cancel_preflight_test")).toEqual([
      { cmd: "cancel_preflight_test", args: { runId: "run-late" } },
    ]);
    expect(h.fake.callsFor("get_preflight_test")).toHaveLength(0);
  });

  it("closes on Escape before a document listener sees it", async () => {
    h = await mount();
    const spy = vi.fn();
    document.addEventListener("keydown", spy);
    try {
      q(h.root, ".close")!.dispatchEvent(escape());
      expect(h.closed()).toBe(1);
      expect(spy).not.toHaveBeenCalled();
    } finally {
      document.removeEventListener("keydown", spy);
    }
  });

  it("closes the open list on the first Escape and the panel on the second", async () => {
    h = await mount();
    const spy = vi.fn();
    document.addEventListener("keydown", spy);
    try {
      input(h).focus();
      expect(list(h).hidden).toBe(false);
      const first = escape();
      input(h).dispatchEvent(first);
      expect(first.defaultPrevented).toBe(true);
      expect(list(h).hidden).toBe(true);
      expect(h.closed()).toBe(0);
      input(h).dispatchEvent(escape());
      expect(h.closed()).toBe(1);
      expect(spy).not.toHaveBeenCalled();
    } finally {
      document.removeEventListener("keydown", spy);
    }
  });

  it("closes the panel on the first Escape after an open picker unmounts", async () => {
    let reply: () => Reply<PreflightTestTarget[]> = () => [row(TARGET)];
    h = await mount({ list: () => reply() });
    input(h).focus();
    expect(list(h).hidden).toBe(false);
    reply = () => [];
    const other = "C:\\Other";
    const room = `${other}\\.ac\\room-2-dev`;
    const more = () => discovery({
      workgroups: [{
        name: "room-2-dev", path: room, task: null,
        agents: [{ name: "ops", path: `${room}\\__agent_ops`, repoPaths: [],
          isCoordinator: false }],
      }],
    });
    h.fake.onInvoke("open_project", () => ({ path: other, ...more() }));
    h.fake.onInvoke("discover_project", () => more());
    await projectStore.loadProject(other);
    await settle();
    expect(q(h.root, ".target")).toBeNull();
    h.root.dispatchEvent(escape());
    expect(h.closed()).toBe(1);
  });

  it("lets an IME composition Escape pass, then handles normal Escapes", async () => {
    h = await mount();
    h.gets.push(snap());
    await startRun(h);
    const composing = escape({ isComposing: true });
    input(h).dispatchEvent(composing);
    const legacy = escape();
    Object.defineProperty(legacy, "keyCode", { value: 229 });
    input(h).dispatchEvent(legacy);
    expect(composing.defaultPrevented).toBe(false);
    expect(legacy.defaultPrevented).toBe(false);
    expect(h.closed()).toBe(0);
    expect(h.fake.callsFor("cancel_preflight_test")).toHaveLength(0);
    input(h).dispatchEvent(escape());
    expect(h.closed()).toBe(1);
    expect(h.fake.callsFor("cancel_preflight_test")).toHaveLength(1);
  });

  it("keeps the IME Escape from closing an open list", async () => {
    h = await mount();
    input(h).focus();
    input(h).dispatchEvent(escape({ isComposing: true }));
    expect(list(h).hidden).toBe(false);
    input(h).dispatchEvent(escape());
    expect(list(h).hidden).toBe(true);
    expect(h.closed()).toBe(0);
  });

  it("marks a reattached run Stale and still polls and cancels it", async () => {
    h = await mount({ describe: () => target({ activeRunId: "run-1", stopped: false }) });
    h.gets.push(snap(), passed());
    await select(h);
    expect(h.fake.lastCall("get_preflight_test")?.args).toEqual({ runId: "run-1" });
    expect(q(h.root, ".cancel")).toBeTruthy();
    expect(status(h)).toBe("Running Coding Agent preflight…");
    await settle(500);
    expect(h.fake.callsFor("get_preflight_test")).toHaveLength(2);
    expect(q(h.root, ".result")!.dataset.acState).toBe("stale");
  });

  it("shows a rejected start, then describes and lists again without a new run", async () => {
    let lists = 0;
    h = await mount({
      list: () => (lists++ < 2 ? [row(TARGET)] : [row(TARGET, { running: true })]),
    });
    h.fake.reject("start_preflight_test", "Settings changed; describe again.");
    await startRun(h);
    expect(h.fake.callsFor("describe_preflight_target")).toHaveLength(2);
    expect(lists).toBe(3);
    expect(h.fake.callsFor("start_preflight_test")).toHaveLength(1);
    expect(status(h)).toBe("Stop one of these agents before testing.");
  });

  it("keeps a backend rejection visible while the list still offers the target", async () => {
    h = await mount();
    h.fake.reject("start_preflight_test", "Settings changed; describe again.");
    await startRun(h);
    expect(status(h)).toBe("Settings changed; describe again.");
  });

  it("stops on a rejected get, keeps Close, and reattaches from describe", async () => {
    let describes = 0;
    h = await mount({
      describe: () => target({ activeRunId: describes++ >= 2 ? "run-2" : null }),
    });
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

  it("shows Loading agents... while the list is pending", async () => {
    const first = deferred<PreflightTestTarget[]>();
    h = await mount({ list: () => first.promise });
    expect(status(h)).toBe("Loading agents...");
    expect(runButton(h)!.disabled).toBe(true);
    first.resolve([row(TARGET)]);
    await settle();
    expect(status(h)).toBe("Which stopped agent should run this test?");
  });

  it("shows a list failure with a Retry that refreshes the list", async () => {
    let reply: () => Reply<PreflightTestTarget[]> = () => new Error("list failed");
    h = await mount({ list: () => reply() });
    expect(status(h)).toBe("list failed");
    expect(runButton(h)!.disabled).toBe(true);
    reply = () => [row(TARGET)];
    q<HTMLButtonElement>(h.root, ".listRetry")!.click();
    await settle();
    expect(status(h)).toBe("Which stopped agent should run this test?");
    expect(q(h.root, ".listRetry")).toBeNull();
  });

  it("ignores a stale list reply", async () => {
    const early = deferred<PreflightTestTarget[]>();
    let calls = 0;
    h = await mount({ list: () => (calls++ === 0 ? early.promise : [row(TARGET)]) });
    expect(calls).toBe(2);
    early.resolve([]);
    await settle();
    expect(options(h)).toHaveLength(1);
  });

  it("clears a selection that is removed or starts running", async () => {
    let rows = [row(TARGET), row(OTHER)];
    h = await mount({ list: () => rows });
    await select(h);
    rows = [row(OTHER)];
    h.fake.reject("start_preflight_test", "Settings changed; describe again.");
    runButton(h)!.click();
    await settle();
    expect(input(h).dataset.acState).toBe("unconfirmed");
    expect(q(h.root, ".info")).toBeNull();
    expect(status(h)).toBe("Which stopped agent should run this test?");
  });

  it("filters by the saved launch Profile and labels fallback and running rows", async () => {
    h = await mount({
      letter: "B",
      list: () => [
        row(TARGET),
        row(OTHER, { requestedProfile: "C", effectiveProfile: "B", fallbackApplied: true }),
        row(`${ROOM}/__agent_ops`, { effectiveProfile: "B", running: true }),
      ],
    });
    expect(q(h.root, ".filter")!.textContent)
      .toBe("Showing only agents that launch with Profile B (saved settings).");
    input(h).focus();
    const [fallback, running] = options(h);
    expect(options(h)).toHaveLength(2);
    expect(fallback.textContent).toContain("qa");
    expect(fallback.textContent).toContain("Asks for Profile C, launches with Profile B.");
    expect(fallback.textContent).not.toContain("Launches with Profile");
    expect(running.textContent).toContain("__agent_ops (running)");
    expect(running.textContent).toContain("Launches with Profile B");
    expect(running.getAttribute("aria-disabled")).toBe("true");
    running.click();
    await settle();
    expect(status(h)).toBe("Which stopped agent should run this test?");
    expect(h.fake.callsFor("describe_preflight_target")).toHaveLength(0);
  });

  it.each([
    ["Z", () => [row(TARGET)],
      "No agent launches with Profile Z. Assign Profile Z to an agent to test it here."],
    ["A", () => [row(TARGET, { running: true })], "Stop one of these agents before testing."],
    [null, () => [],
      "No compatible stopped agent is available. Create or stop an agent before testing."],
  ])("explains an unusable list for letter %s", async (letter, rows, text) => {
    h = await mount({ letter, list: rows });
    expect(status(h)).toBe(text);
    expect(runButton(h)!.disabled).toBe(true);
  });

  it("shows every row in card mode with its launch Profile", async () => {
    h = await mount({ list: () => [row(TARGET), row(OTHER, { effectiveProfile: "B" })] });
    expect(q(h.root, ".filter")).toBeNull();
    input(h).focus();
    expect(options(h).map((o) => o.textContent)).toEqual([
      `__agent_dev${TARGET}Launches with Profile A`,
      `__agent_qa${OTHER}Launches with Profile B`,
    ]);
  });
});
