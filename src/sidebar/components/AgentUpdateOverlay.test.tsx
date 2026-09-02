// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import { toastStore } from "../../shared/stores/toasts";
import type { UnlistenFn } from "../../shared/transport";
import type {
  AgentUpdateCommandRef,
  AgentUpdateNode,
  AgentUpdatePrompt,
  AgentUpdateResult,
  AgentUpdateStatus,
  InstallState,
} from "../../shared/types";
import { agentUpdateStore, resetAgentUpdateForTests, wireAgentUpdateListeners } from "../agent-update";
import AgentUpdateOverlay from "./AgentUpdateOverlay";

const [store, setAgentUpdateStore] = agentUpdateStore;

// Frozen: a Solid path write (`set("prompt", next)`) merges `next` INTO the object the store
// already holds; the store copies a frozen object, so the shared constant is never mutated.
const PROMPT: AgentUpdatePrompt = Object.freeze({ command: "claude", label: "Claude" });

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing ${testId}`);
  return element;
}

function query(testId: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/** One macrotask: every pending microtask (an answer and its snapshot read included) has run. */
async function settle(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

function installed(version: string, seq: number): InstallState {
  return { status: "installed", version, path: `C:\\bin\\${version}.cmd`, seq };
}

function missing(seq: number): InstallState {
  return { status: "missing", detail: "'x' was not found on PATH", seq };
}

function node(command: string, label: string, installBefore?: InstallState, updateCommands?: string[]): AgentUpdateNode {
  const base = { command, label, updateCommands: updateCommands ?? [`${command} update`] };
  return installBefore ? { ...base, installBefore } : base;
}

/** #1691 - the canonical succeeded result: both probe keys present, no version claim. */
function ok(command: string, label: string): AgentUpdateResult {
  return {
    command,
    label,
    ok: true,
    outcome: "succeeded",
    installBefore: null,
    installAfter: null,
    change: "unknown",
  };
}

function failed(command: string, label: string, error: string): AgentUpdateResult {
  return { ...ok(command, label), ok: false, outcome: "failed", error };
}

/** #1691 - `ok=false` and never a failure. */
function cancelled(command: string, label: string): AgentUpdateResult {
  return { ...ok(command, label), ok: false, outcome: "cancelled" };
}

function changed(command: string, label: string, before: string, after: string): AgentUpdateResult {
  return {
    ...ok(command, label),
    change: "changed",
    installBefore: installed(before, 0),
    installAfter: installed(after, 1),
  };
}

function unchanged(command: string, label: string, version: string): AgentUpdateResult {
  return {
    ...ok(command, label),
    change: "unchanged",
    installBefore: installed(version, 0),
    installAfter: installed(version, 1),
  };
}

function ref(command: string, label: string): AgentUpdateCommandRef {
  return { command, label };
}

function status(overrides: Partial<AgentUpdateStatus> = {}): AgentUpdateStatus {
  return {
    inProgress: true,
    prompt: null,
    results: [],
    running: [],
    verifying: [],
    cancelRequested: [],
    cancelAllRequested: false,
    ...overrides,
  };
}

function keydown(key: "Enter" | "Escape"): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
  document.dispatchEvent(event);
  return event;
}

function click(testId: string): void {
  byTestId(testId).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
}

function nodeItems(): HTMLElement[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="agent-update.node."][data-ac-role="listitem"]')
  );
}

describe("AgentUpdateOverlay (#1327, #1551)", () => {
  let fake: FakeTransport;
  let restore: () => void;
  let dispose: (() => void) | null = null;
  let unlisteners: UnlistenFn[] = [];

  /** Wire the store's listeners on the fake exactly as the sidebar does (snapshot `null` unless given). */
  async function wire(snapshot: AgentUpdateStatus | null = null): Promise<void> {
    fake.resolve("get_agent_update_status", snapshot);
    unlisteners.push(...(await wireAgentUpdateListeners()));
  }

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    resetAgentUpdateForTests();
    const root = document.createElement("div");
    document.body.appendChild(root);
    dispose = render(() => <AgentUpdateOverlay />, root);
  });

  afterEach(() => {
    dispose?.();
    dispose = null;
    for (const unlisten of unlisteners.splice(0)) unlisten();
    unlisteners = [];
    toastStore.clear();
    resetAgentUpdateForTests();
    document.body.replaceChildren();
    restore();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("shows the splash while in progress and the prompt modal when a prompt is pending", () => {
    setAgentUpdateStore({ inProgress: true, prompt: null });
    const overlay = byTestId("agent-update.overlay");
    expect(overlay.textContent).toContain("Updating coding agents...");
    expect(overlay.getAttribute("data-ac-state")).toBe("pass");

    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });
    expect(overlay.textContent).toContain("Automatically update the Claude coding agent at startup?");
    expect(overlay.getAttribute("data-ac-state")).toBe("prompt");
    expect(byTestId("agent-update.prompt.yes").textContent).toBe("Yes");
    expect(byTestId("agent-update.prompt.no").textContent).toBe("No");
  });

  it("Yes answer applied this boot closes the modal, reads no snapshot and runs nothing else", async () => {
    fake.resolve("agent_update_answer", true);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.yes");
    await settle();

    expect(store.prompt).toBeNull();
    expect(fake.lastCall("agent_update_answer")?.args).toEqual({
      command: "claude",
      enabled: true,
    });
    expect(fake.callsFor("get_agent_update_status")).toHaveLength(0);
    expect(toastStore.items).toHaveLength(0);
    expect(store.closedPrompts).toEqual(["claude"]);
  });

  it("late Yes answer closes the modal AND toasts the pinned conditional info text", async () => {
    fake.resolve("agent_update_answer", false);
    fake.resolve("get_agent_update_status", status({ answered: { claude: true } }));
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.yes");
    await settle();

    expect(store.prompt).toBeNull();
    expect(fake.callsFor("get_agent_update_status")).toHaveLength(1);
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("info");
    expect(toastStore.items[0].message).toBe("This coding agent will be updated at the next startup.");
  });

  it("late No answer closes the modal AND toasts the conditional No text", async () => {
    fake.resolve("agent_update_answer", false);
    fake.resolve("get_agent_update_status", status({ answered: { claude: false } }));
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.no");
    await settle();

    expect(store.prompt).toBeNull();
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("You will not be asked again.");
  });

  it("IPC failure keeps the modal open and toasts the error (retry)", async () => {
    fake.reject("agent_update_answer", "settings lock contention");
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.no");
    await settle();

    expect(store.prompt).toEqual(PROMPT); // still open
    expect(store.closedPrompts).toEqual([]);
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("error");
  });

  it("R2: while the answer IPC is in flight, Enter/Esc and a second click are ignored", async () => {
    const pending = deferred<boolean>();
    fake.onInvoke("agent_update_answer", () => pending.promise);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.yes");
    // In flight: a second click, Enter, and Esc must NOT fire another answer.
    click("agent-update.prompt.yes");
    keydown("Enter");
    keydown("Escape");
    await Promise.resolve();

    expect(fake.callsFor("agent_update_answer")).toHaveLength(1);
    expect(store.prompt).toEqual(PROMPT); // still open while in flight

    pending.resolve(true);
    await settle();
    expect(store.prompt).toBeNull();
    expect(fake.callsFor("agent_update_answer")).toHaveLength(1);
  });

  it("Enter and Esc answer No by default", async () => {
    fake.resolve("agent_update_answer", true);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    keydown("Escape");
    await settle();

    expect(fake.lastCall("agent_update_answer")?.args).toEqual({
      command: "claude",
      enabled: false,
    });
  });

  it("the timeline renders one node per pass node in order with data-ac-state and the state text", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("a", "A"), node("b", "B"), node("c", "C")],
      running: [{ command: "b", label: "B" }],
      results: [ok("a", "A")],
    });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("pass");
    expect(document.querySelector(".agent-update-card--timeline")).toBeTruthy();
    expect(byTestId("agent-update.header")).toBeTruthy();
    expect(byTestId("agent-update.timeline").getAttribute("aria-label")).toBe("Coding agent updates");
    expect(byTestId("agent-update.timeline").getAttribute("data-ac-role")).toBe("list");

    const items = nodeItems();
    expect(items.map((li) => li.getAttribute("data-ac-testid"))).toEqual([
      "agent-update.node.a",
      "agent-update.node.b",
      "agent-update.node.c",
    ]);
    expect(items.map((li) => li.getAttribute("data-ac-state"))).toEqual(["ok", "updating", "pending"]);
    expect(items.map((li) => li.getAttribute("data-ac-command"))).toEqual(["a", "b", "c"]);
    // #1691 - a terminal row carries NO state word: only the nonterminal rows have one
    expect(query("agent-update.node.a.state")).toBeNull();
    expect(["b", "c"].map((command) => byTestId(`agent-update.node.${command}.state`).textContent)).toEqual([
      "Updating...",
      "Pending",
    ]);
    expect(byTestId("agent-update.node.a.detail").textContent).toBe(
      "Update completed - Version could not be verified"
    );
    expect(items.map((li) => li.querySelector(".agent-update-node-label")!.textContent)).toEqual(["A", "B", "C"]);
    // markers: a glyph for the terminal states only
    expect(items[0].querySelector(".agent-update-node-marker svg")).toBeTruthy();
    expect(items[1].querySelector(".agent-update-node-marker svg")).toBeNull();
    expect(items[2].querySelector(".agent-update-node-marker svg")).toBeNull();
    expect(items[0].querySelector(".agent-update-node-marker")!.getAttribute("aria-hidden")).toBe("true");
  });

  it("only the running node prints its update commands", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("a", "A"), node("b", "B", undefined, ["b step 1", "b step 2"]), node("c", "C")],
      running: [{ command: "b", label: "B" }],
      results: [ok("a", "A")],
    });
    const command = byTestId("agent-update.node.b.command");
    expect(Array.from(command.querySelectorAll("code")).map((code) => code.textContent)).toEqual([
      "b step 1",
      "b step 2",
    ]);
    expect(byTestId("agent-update.node.b").querySelector(".agent-update-node-bar")).toBeTruthy();
    expect(byTestId("agent-update.node.b").hasAttribute("title")).toBe(false);

    expect(query("agent-update.node.a.command")).toBeNull();
    expect(query("agent-update.node.c.command")).toBeNull();
    expect(byTestId("agent-update.node.a").querySelector(".agent-update-node-bar")).toBeNull();
    expect(byTestId("agent-update.node.a").getAttribute("title")).toBe("a update");
    expect(byTestId("agent-update.node.c").getAttribute("title")).toBe("c update");
  });

  it("#1691 - every terminal row is ONE string with no separator, from the result's own fields", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [
        node("a", "A", installed("1.0", 0)),
        node("b", "B", installed("1.0", 0)),
        node("c", "C"),
        node("d", "D"),
        node("e", "E"),
        node("f", "F"),
      ],
      results: [
        changed("a", "A", "1.2.3", "1.2.4"),
        failed("b", "B", "exit code 1"),
        unchanged("c", "C", "1.2.3"),
        ok("d", "D"),
        cancelled("e", "E"),
        { ...failed("f", "F", ""), error: null },
      ],
      // the one-shot install cache is NOT a source for any of these strings
      installAfter: { a: installed("9.9", 2), b: missing(3) },
    });

    const line = (command: string) =>
      byTestId(`agent-update.node.${command}`).querySelector(".agent-update-node-line")!.textContent;
    expect(line("a")).toBe("Ready - 1.2.3 -> 1.2.4");
    expect(line("b")).toBe("Failed - exit code 1");
    expect(line("c")).toBe("1.2.3 (Nothing to update)");
    expect(line("d")).toBe("Update completed - Version could not be verified");
    expect(line("e")).toBe("Cancelled");
    expect(line("f")).toBe("Failed");

    // the detail carries the whole string and its own title
    expect(byTestId("agent-update.node.a.detail").textContent).toBe("Ready - 1.2.3 -> 1.2.4");
    expect(byTestId("agent-update.node.a.detail").getAttribute("title")).toBe("Ready - 1.2.3 -> 1.2.4");

    // a terminal row has NO state word and no separator element at all
    for (const command of ["a", "b", "c", "d", "e", "f"]) {
      const item = byTestId(`agent-update.node.${command}`);
      expect(query(`agent-update.node.${command}.state`)).toBeNull();
      expect(item.querySelectorAll(".agent-update-node-line")).toHaveLength(1);
      expect(item.querySelector(".agent-update-node-sep")).toBeNull();
      expect(item.querySelector(".agent-update-node-head")).toBeNull();
      expect(item.textContent).not.toContain(" · ");
      expect(item.textContent).not.toContain("→");
    }

    // states and markers: a cross for failed AND cancelled, a check for ok
    expect(["a", "b", "c", "d", "e", "f"].map((command) =>
      byTestId(`agent-update.node.${command}`).getAttribute("data-ac-state")
    )).toEqual(["ok", "failed", "ok", "ok", "cancelled", "failed"]);
    expect(byTestId("agent-update.node.b").querySelector(".agent-update-node-marker svg")).toBeTruthy();
    expect(byTestId("agent-update.node.e").querySelector(".agent-update-node-marker svg")).toBeTruthy();
  });

  it("the header counter, attributes and progressbar follow the counts", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("a", "A"), node("b", "B"), node("c", "C")],
      results: [ok("a", "A")],
    });
    const text = byTestId("agent-update.progress.text");
    expect(text.textContent).toBe("1 of 3 completed");
    expect(text.getAttribute("data-ac-done")).toBe("1");
    expect(text.getAttribute("data-ac-total")).toBe("3");
    expect(text.getAttribute("data-ac-failed")).toBe("0");
    const bar = byTestId("agent-update.progress");
    expect(bar.getAttribute("role")).toBe("progressbar");
    expect(bar.getAttribute("aria-label")).toBe("Coding agents completed");
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuenow")).toBe("1");
    expect(bar.getAttribute("aria-valuemax")).toBe("3");
    expect(bar.querySelector<HTMLElement>(".agent-update-progress-fill")!.style.width).toBe("33%");

    setAgentUpdateStore("results", [ok("a", "A"), failed("b", "B", "exit code 1")]);
    expect(text.textContent).toBe("2 of 3 completed, 1 failed");
    expect(text.getAttribute("data-ac-failed")).toBe("1");
    expect(bar.getAttribute("aria-valuenow")).toBe("2");
    expect(bar.querySelector<HTMLElement>(".agent-update-progress-fill")!.style.width).toBe("67%");
  });

  it("#1691 - the prompt callout, the timeline and both controls coexist in the required DOM order", () => {
    setAgentUpdateStore({
      inProgress: true,
      prompt: PROMPT,
      nodes: [node("a", "A"), node("b", "B")],
      running: [ref("a", "A")],
    });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("prompt");
    // the card no longer chooses prompt OR timeline
    expect(document.querySelector(".agent-update-card--timeline")).toBeTruthy();
    expect(byTestId("agent-update.header")).toBeTruthy();
    expect(byTestId("agent-update.progress")).toBeTruthy();
    expect(byTestId("agent-update.progress.text")).toBeTruthy();
    expect(byTestId("agent-update.timeline")).toBeTruthy();
    expect(byTestId("agent-update.prompt.yes")).toBeTruthy();
    expect(byTestId("agent-update.prompt.no")).toBeTruthy();
    expect(byTestId("agent-update.node.a.cancel")).toBeTruthy();
    expect(byTestId("agent-update.cancel-all")).toBeTruthy();

    // header/progress, then the prompt callout, then the timeline, then Cancel all
    const card = document.querySelector(".agent-update-card")!;
    const order = Array.from(card.children).map((child) => child.className.split(" ")[0]);
    expect(order).toEqual([
      "agent-update-header",
      "agent-update-progress",
      "agent-update-prompt",
      "agent-update-timeline",
      "agent-update-batch-actions",
    ]);
    // the callout holds the question and both answers
    const callout = card.querySelector(".agent-update-prompt")!;
    expect(callout.textContent).toContain("Automatically update the Claude coding agent at startup?");
    expect(callout.contains(byTestId("agent-update.prompt.no"))).toBe(true);

    resetAgentUpdateForTests();
    setAgentUpdateStore({ inProgress: true });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("pass");
    expect(document.querySelector(".agent-update-spinner")).toBeTruthy();
    expect(byTestId("agent-update.title").textContent).toBe("Updating coding agents...");
    expect(query("agent-update.timeline")).toBeNull();
    expect(query("agent-update.progress.text")).toBeNull();
    expect(query("agent-update.progress")).toBeNull();
    expect(query("agent-update.summary.close")).toBeNull();
  });

  it("finished enters the summary and it persists until Close", async () => {
    vi.useFakeTimers();
    await wire();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("a", "A"), node("b", "B")] });
    fake.emitFromBackend("agent_update_command_finished", ok("a", "A"));
    fake.emitFromBackend("agent_update_command_finished", failed("b", "B", "exit code 1"));
    fake.emitFromBackend("agent_updates_finished", { results: [ok("a", "A"), failed("b", "B", "exit code 1")] });

    const overlay = byTestId("agent-update.overlay");
    expect(overlay.getAttribute("data-ac-state")).toBe("summary");
    expect(byTestId("agent-update.title").textContent).toBe("Coding agent updates complete");
    expect(byTestId("agent-update.done").querySelector("svg")).toBeTruthy();
    expect(byTestId("agent-update.done").getAttribute("aria-hidden")).toBe("true");
    expect(document.querySelector(".agent-update-spinner")).toBeNull();
    expect(byTestId("agent-update.progress.text").textContent).toBe("2 of 2 completed, 1 failed");
    expect(byTestId("agent-update.progress").getAttribute("aria-valuenow")).toBe("2");
    expect(nodeItems().map((li) => li.getAttribute("data-ac-state"))).toEqual(["ok", "failed"]);
    const close = byTestId<HTMLButtonElement>("agent-update.summary.close");
    expect(close.textContent).toBe("Close");
    expect(document.activeElement).toBe(close);

    vi.advanceTimersByTime(60_000);
    expect(query("agent-update.overlay")).toBeTruthy();
    expect(store.summary).toBe("shown");

    close.click();
    expect(query("agent-update.overlay")).toBeNull();
    expect(store.summary).toBe("dismissed");
  });

  it("Escape and Enter close the summary and send no answer", async () => {
    await wire();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("a", "A")] });
    fake.emitFromBackend("agent_updates_finished", { results: [ok("a", "A")] });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("summary");

    const escape = keydown("Escape");
    expect(escape.defaultPrevented).toBe(true);
    expect(query("agent-update.overlay")).toBeNull();
    expect(store.summary).toBe("dismissed");
    expect(fake.callsFor("agent_update_answer")).toHaveLength(0);

    // a new pass on this surface: Enter closes too, and the button is never activated natively
    fake.emitFromBackend("agent_updates_started", { nodes: [node("a", "A")] });
    fake.emitFromBackend("agent_updates_finished", { results: [ok("a", "A")] });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("summary");
    const enter = keydown("Enter");
    expect(enter.defaultPrevented).toBe(true);
    expect(query("agent-update.overlay")).toBeNull();
    expect(store.summary).toBe("dismissed");
    expect(fake.callsFor("agent_update_answer")).toHaveLength(0);
    // a second Enter on the dismissed summary is inert
    keydown("Enter");
    expect(store.summary).toBe("dismissed");
  });

  it("Enter/Escape still answer No while a prompt is shown, with nodes present", async () => {
    fake.resolve("agent_update_answer", true);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT, nodes: [node("a", "A")] });

    keydown("Escape");
    await settle();

    expect(fake.lastCall("agent_update_answer")?.args).toEqual({ command: "claude", enabled: false });
    expect(fake.callsFor("agent_update_answer")).toHaveLength(1);
    expect(store.prompt).toBeNull();
    // the pass is still running on this surface: the timeline is back
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("pass");
    expect(byTestId("agent-update.node.a.state").textContent).toBe("Pending");
  });

  it("the failure toast appears on close, not on finished", async () => {
    await wire();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("bad-stub", "Bad")] });
    fake.emitFromBackend("agent_updates_finished", { results: [failed("bad-stub", "Bad", "exit code 1")] });
    expect(store.summary).toBe("shown");
    expect(toastStore.items).toHaveLength(0);
    expect(byTestId("agent-update.node.bad-stub").querySelector(".agent-update-node-line")!.textContent).toBe(
      "Failed - exit code 1"
    );

    click("agent-update.summary.close");
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("error");
    expect(toastStore.items[0].message).toBe("Auto-update failed for Bad (bad-stub): exit code 1");
  });

  it("a finished pass with zero results closes the overlay without a summary", async () => {
    await wire();
    fake.emitFromBackend("agent_updates_started", { nodes: [] });
    expect(query("agent-update.overlay")).toBeTruthy();
    fake.emitFromBackend("agent_updates_finished", { results: [] });
    expect(query("agent-update.overlay")).toBeNull();
    expect(store.summary).toBe("none");
    expect(toastStore.items).toHaveLength(0);
  });

  it("a surface that mounts after the pass shows nothing", async () => {
    await wire(
      status({
        inProgress: false,
        results: [failed("bad-stub", "Bad", "exit code 1")],
        nodes: [node("bad-stub", "Bad")],
      })
    );
    await settle();
    expect(query("agent-update.overlay")).toBeNull();
    expect(store.summary).toBe("none");
    // a genuine post-pass mount keeps the immediate toast
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("Auto-update failed for Bad (bad-stub): exit code 1");
  });

  it("#1691 - install_state_changed after finished never rewrites a terminal row's outcome", async () => {
    await wire();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("a", "A", installed("1.0", 0))] });
    fake.emitFromBackend("agent_updates_finished", { results: [unchanged("a", "A", "1.0")] });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("summary");
    const detail = byTestId("agent-update.node.a.detail");
    expect(detail.textContent).toBe("1.0 (Nothing to update)");

    // the cache still records the probe...
    fake.emitFromBackend("agent_install_state_changed", { command: "a", install: installed("1.1", 1) });
    expect(store.installAfter.a).toEqual(installed("1.1", 1));
    // ...but the row's text comes from the result alone, and the <li> is stable (<Index>)
    expect(byTestId("agent-update.node.a.detail")).toBe(detail);
    expect(detail.textContent).toBe("1.0 (Nothing to update)");
    expect(detail.getAttribute("title")).toBe("1.0 (Nothing to update)");
  });

  it("the title carries role=status and aria-live=polite", () => {
    setAgentUpdateStore({ inProgress: true });
    const title = byTestId("agent-update.title");
    expect(title.getAttribute("role")).toBe("status");
    expect(title.getAttribute("aria-live")).toBe("polite");
    expect(title.classList.contains("agent-update-text")).toBe(true);
  });

  it("the Pending state word keeps the AA token while the label stays dimmed", () => {
    setAgentUpdateStore({ inProgress: true, nodes: [node("a", "A")] });
    const stateWord = byTestId("agent-update.node.a.state");
    expect(stateWord.classList.contains("agent-update-node-state")).toBe(true);
    const item = stateWord.closest("li")!;
    expect(item.getAttribute("data-ac-state")).toBe("pending");
    expect(item.classList.contains("agent-update-node")).toBe(true);
    expect(item.querySelector(".agent-update-node-label")!.textContent).toBe("A");

    // Vite rewrites the literal-string form of new URL(..., import.meta.url) into a served asset
    // URL (http://localhost:3000/...) under the jsdom environment; a variable base keeps the real
    // file: URL of this module, which node:fs accepts.
    const moduleUrl = import.meta.url;
    const css = readFileSync(new URL("../styles/sidebar.css", moduleUrl), "utf8");
    const occurrences = (rule: string) => css.split(rule).length - 1;
    expect(
      occurrences('.agent-update-node[data-ac-state="pending"] .agent-update-node-state { color: var(--sidebar-fg); }')
    ).toBe(1);
    expect(
      occurrences(
        '.agent-update-node[data-ac-state="pending"] .agent-update-node-label { color: var(--sidebar-fg-dim); font-weight: 400; }'
      )
    ).toBe(1);
  });

  it("a superseded answer toasts the policy the other surface persisted", async () => {
    fake.resolve("agent_update_answer", false);
    fake.resolve("get_agent_update_status", status({ answered: { claude: false } }));
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.yes");
    await settle();

    expect(store.prompt).toBeNull();
    expect(query("agent-update.prompt.yes")).toBeNull();
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("You will not be asked again.");
    expect(store.closedPrompts).toContain("claude");
  });

  it("a false answer without a recorded policy toasts nothing", async () => {
    let statusMode: "empty" | "reject" = "empty";
    fake.resolve("agent_update_answer", false);
    fake.onInvoke("get_agent_update_status", () =>
      statusMode === "reject" ? Promise.reject(new Error("boom")) : Promise.resolve(status({ answered: {} }))
    );
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });
    click("agent-update.prompt.no");
    await settle();
    expect(store.prompt).toBeNull();
    expect(toastStore.items).toHaveLength(0);
    expect(errorSpy).not.toHaveBeenCalled();

    // the snapshot read fails: the modal stays closed, nothing is toasted, one console.error
    resetAgentUpdateForTests();
    statusMode = "reject";
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });
    click("agent-update.prompt.yes");
    await settle();
    expect(store.prompt).toBeNull();
    expect(query("agent-update.prompt.yes")).toBeNull();
    expect(toastStore.items).toHaveLength(0);
    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(String(errorSpy.mock.calls[0][0])).toContain("[agent-update] getStatus after answer failed:");
  });

  it("the prompt closed by the store sends no answer on Escape", async () => {
    fake.resolve("agent_update_answer", true);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });
    expect(byTestId("agent-update.prompt.no")).toBeTruthy();

    setAgentUpdateStore("prompt", null); // what prompt_closed does on this surface
    expect(query("agent-update.prompt.no")).toBeNull();
    keydown("Escape");
    await settle();

    expect(fake.callsFor("agent_update_answer")).toHaveLength(0);
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("pass");
  });

  it("answer(A) resolving after prompt B arrived leaves B visible", async () => {
    const pending = deferred<boolean>();
    fake.onInvoke("agent_update_answer", () => pending.promise);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.yes");
    expect(fake.callsFor("agent_update_answer")).toHaveLength(1);
    expect(fake.lastCall("agent_update_answer")?.args).toEqual({ command: "claude", enabled: true });
    expect(byTestId<HTMLButtonElement>("agent-update.prompt.yes").disabled).toBe(true);

    // prompt B arrives while A's answer is in flight
    setAgentUpdateStore("prompt", { command: "codex", label: "Codex" });
    expect(byTestId("agent-update.overlay").textContent).toContain("Automatically update the Codex coding agent at startup?");

    pending.resolve(true);
    await settle();

    expect(store.prompt).toEqual({ command: "codex", label: "Codex" });
    expect(byTestId("agent-update.overlay").textContent).toContain("Automatically update the Codex coding agent at startup?");
    expect(byTestId<HTMLButtonElement>("agent-update.prompt.yes").disabled).toBe(false);
    expect(byTestId<HTMLButtonElement>("agent-update.prompt.no").disabled).toBe(false);
    expect(store.closedPrompts).toContain("claude");
    expect(store.closedPrompts).not.toContain("codex");
    expect(toastStore.items).toHaveLength(0);
    expect(fake.callsFor("agent_update_answer")).toHaveLength(1);
  });
});

describe("AgentUpdateOverlay cancellation controls (#1691)", () => {
  let fake: FakeTransport;
  let restore: () => void;
  let dispose: (() => void) | null = null;
  let unlisteners: UnlistenFn[] = [];
  let errorSpy: ReturnType<typeof vi.spyOn>;

  function mount(): void {
    const root = document.createElement("div");
    document.body.appendChild(root);
    dispose = render(() => <AgentUpdateOverlay />, root);
  }

  const PASS_NODES = () => [node("a", "A"), node("b", "B"), node("c", "C")];

  /**
   * A pass with three nonterminal rows: running `a`, verifying `b`, pending `c`.
   * It registers NO `get_agent_update_status` handler, so each test decides whether the
   * post-response hydration resolves (`resolveHydration`) or rejects.
   */
  function seedPass(): void {
    setAgentUpdateStore({
      inProgress: true,
      nodes: PASS_NODES(),
      running: [ref("a", "A")],
      verifying: [ref("b", "B")],
    });
  }

  /** The snapshot the backend would really return mid-pass: the same node set. */
  function resolveHydration(): void {
    fake.resolve(
      "get_agent_update_status",
      status({ inProgress: true, nodes: PASS_NODES(), running: [ref("a", "A")], verifying: [ref("b", "B")] })
    );
  }

  function cancelButtons(): HTMLButtonElement[] {
    return Array.from(
      document.querySelectorAll<HTMLButtonElement>('[data-ac-testid$=".cancel"][data-ac-cancel="row"]')
    );
  }

  function keydownOn(element: HTMLElement, key: "Enter" | "Escape" | " "): KeyboardEvent {
    const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    element.dispatchEvent(event);
    return event;
  }

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    resetAgentUpdateForTests();
    errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    mount();
  });

  afterEach(() => {
    dispose?.();
    dispose = null;
    for (const unlisten of unlisteners.splice(0)) unlisten();
    unlisteners = [];
    toastStore.clear();
    resetAgentUpdateForTests();
    document.body.replaceChildren();
    restore();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // presence, accessible names and stable ids
  // -------------------------------------------------------------------------

  it("shows one row control per nonterminal row, verifying included, and none for a terminal row", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("a", "A"), node("b", "B"), node("c", "C"), node("d", "D")],
      running: [ref("a", "A")],
      verifying: [ref("b", "B")],
      results: [ok("d", "D")],
    });
    // pending c, running a, verifying b -> three controls; terminal d -> none
    expect(cancelButtons().map((button) => button.dataset.acTestid)).toEqual([
      "agent-update.node.a.cancel",
      "agent-update.node.b.cancel",
      "agent-update.node.c.cancel",
    ]);
    expect(query("agent-update.node.d.cancel")).toBeNull();

    // the verifying row shows the exact word and is still cancellable
    expect(byTestId("agent-update.node.b.state").textContent).toBe("Verifying...");
    expect(byTestId<HTMLButtonElement>("agent-update.node.b.cancel").disabled).toBe(false);

    // accessible names, type and the batch control's name
    expect(byTestId("agent-update.node.a.cancel").getAttribute("aria-label")).toBe("Cancel A update");
    expect(byTestId("agent-update.node.b.cancel").getAttribute("aria-label")).toBe("Cancel B update");
    for (const button of cancelButtons()) expect(button.getAttribute("type")).toBe("button");
    const batch = byTestId<HTMLButtonElement>("agent-update.cancel-all");
    expect(batch.getAttribute("aria-label")).toBe("Cancel all coding agent updates");
    expect(batch.getAttribute("type")).toBe("button");
    expect(batch.textContent).toBe("Cancel all");
    expect(batch.disabled).toBe(false);
  });

  it("a verifying-only remainder still leaves the batch control visible and actionable", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("a", "A"), node("b", "B")],
      verifying: [ref("b", "B")],
      results: [ok("a", "A")],
    });
    const batch = byTestId<HTMLButtonElement>("agent-update.cancel-all");
    expect(batch.disabled).toBe(false);
    expect(cancelButtons().map((button) => button.dataset.acTestid)).toEqual([
      "agent-update.node.b.cancel",
    ]);

    // once every row is terminal the batch control is gone
    setAgentUpdateStore({ verifying: [], results: [ok("a", "A"), cancelled("b", "B")] });
    expect(query("agent-update.cancel-all")).toBeNull();
    expect(cancelButtons()).toHaveLength(0);
  });

  it("every pre-existing stable test id is still queryable", () => {
    setAgentUpdateStore({
      inProgress: true,
      prompt: PROMPT,
      nodes: [node("a", "A"), node("b", "B")],
      running: [ref("a", "A")],
      results: [ok("b", "B")],
    });
    for (const id of [
      "agent-update.overlay",
      "agent-update.header",
      "agent-update.title",
      "agent-update.progress",
      "agent-update.progress.text",
      "agent-update.timeline",
      "agent-update.node.a",
      "agent-update.node.a.state",
      "agent-update.node.a.command",
      "agent-update.node.b",
      "agent-update.node.b.detail",
      "agent-update.prompt.yes",
      "agent-update.prompt.no",
    ]) {
      expect(byTestId(id)).toBeTruthy();
    }

    setAgentUpdateStore({ prompt: null, summary: "shown" });
    expect(byTestId("agent-update.done")).toBeTruthy();
    expect(byTestId("agent-update.summary.close")).toBeTruthy();
  });

  it("no rendered string, ARIA label or toast carries a Spanish literal", async () => {
    fake.resolve("agent_update_cancel", { command: "a", disposition: "requested" });
    fake.reject("get_agent_update_status", "boom");
    setAgentUpdateStore({
      inProgress: true,
      prompt: PROMPT,
      nodes: [node("a", "A"), node("b", "B"), node("c", "C")],
      running: [ref("a", "A")],
      verifying: [ref("b", "B")],
      results: [cancelled("c", "C")],
    });
    click("agent-update.node.a.cancel");
    await settle();

    const overlay = byTestId("agent-update.overlay");
    const aria = Array.from(overlay.querySelectorAll("[aria-label]")).map((element) =>
      element.getAttribute("aria-label")
    );
    const rendered = [overlay.textContent ?? "", ...aria, ...toastStore.items.map((item) => item.message)];
    for (const value of rendered) {
      expect(value).not.toMatch(/[áéíóúñ¿¡]/);
      expect(value).not.toMatch(/\b(Sí|Cerrar|Pendiente|Actualizando|completados|Listo|actualizados)\b/);
      expect(value).not.toContain("→");
      expect(value).not.toContain("·");
    }
    expect(aria).toContain("Coding agents completed");
    expect(byTestId("agent-update.timeline").getAttribute("aria-label")).toBe("Coding agent updates");
  });

  it("renders the exact nonterminal and terminal DOM copy of every state", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("p", "P"), node("u", "U"), node("v", "V"), node("x", "X"), node("ch", "Ch"), node("un", "Un"), node("k", "K"), node("ca", "Ca"), node("f", "F")],
      running: [ref("u", "U")],
      verifying: [ref("v", "V"), ref("x", "X")],
      cancelRequested: [ref("x", "X")],
      results: [
        changed("ch", "Ch", "1.2.3", "1.2.4"),
        unchanged("un", "Un", "1.2.3"),
        ok("k", "K"),
        cancelled("ca", "Ca"),
        failed("f", "F", "exit code 1"),
      ],
    });
    const stateOf = (command: string) => byTestId(`agent-update.node.${command}.state`).textContent;
    const detailOf = (command: string) => byTestId(`agent-update.node.${command}.detail`).textContent;
    expect(stateOf("p")).toBe("Pending");
    expect(stateOf("u")).toBe("Updating...");
    expect(stateOf("v")).toBe("Verifying...");
    expect(stateOf("x")).toBe("Cancelling...");
    expect(detailOf("ch")).toBe("Ready - 1.2.3 -> 1.2.4");
    expect(detailOf("un")).toBe("1.2.3 (Nothing to update)");
    expect(detailOf("k")).toBe("Update completed - Version could not be verified");
    expect(detailOf("ca")).toBe("Cancelled");
    expect(detailOf("f")).toBe("Failed - exit code 1");
    // the header counts every terminal row as done and only `failed` as failed
    expect(byTestId("agent-update.progress.text").textContent).toBe("5 of 9 completed, 1 failed");
  });

  // -------------------------------------------------------------------------
  // interaction: pointer, Enter, Space; and the prompt's own keys
  // -------------------------------------------------------------------------

  it("a pointer click cancels exactly once and sends no answer", async () => {
    fake.resolve("agent_update_cancel", { command: "a", disposition: "requested" });
    resolveHydration();
    seedPass();
    setAgentUpdateStore("prompt", PROMPT);

    click("agent-update.node.a.cancel");
    await settle();

    expect(fake.callsFor("agent_update_cancel")).toHaveLength(1);
    expect(fake.lastCall("agent_update_cancel")?.args).toEqual({ command: "a" });
    expect(fake.callsFor("agent_update_answer")).toHaveLength(0);
    // the prompt was neither answered nor transiently rendered as No
    expect(store.prompt).toEqual(PROMPT);
    expect(store.closedPrompts).toEqual([]);
    expect(byTestId("agent-update.prompt.no")).toBeTruthy();
  });

  it("Enter on a focused cancel control is NOT captured: the prompt is never answered", async () => {
    fake.resolve("agent_update_cancel", { command: "a", disposition: "requested" });
    resolveHydration();
    seedPass();
    setAgentUpdateStore("prompt", PROMPT);
    const button = byTestId<HTMLButtonElement>("agent-update.node.a.cancel");
    button.focus();

    const event = keydownOn(button, "Enter");
    // the document capture handler stepped aside: it neither prevented the default
    // (which is what activates the button natively) nor answered the prompt
    expect(event.defaultPrevented).toBe(false);
    expect(fake.callsFor("agent_update_answer")).toHaveLength(0);
    expect(store.prompt).toEqual(PROMPT);

    // the native activation the browser performs for that Enter
    button.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await settle();
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(1);
    expect(fake.callsFor("agent_update_answer")).toHaveLength(0);
    expect(store.closedPrompts).toEqual([]);
  });

  it("Space is never captured, so a focused control activates natively and answers nothing", async () => {
    fake.resolve("agent_updates_cancel_all", {
      requested: [ref("a", "A")],
      alreadyRequested: [],
      alreadyTerminal: [],
    });
    resolveHydration();
    seedPass();
    setAgentUpdateStore("prompt", PROMPT);
    const batch = byTestId<HTMLButtonElement>("agent-update.cancel-all");
    batch.focus();

    const event = keydownOn(batch, " ");
    expect(event.defaultPrevented).toBe(false);
    expect(fake.callsFor("agent_update_answer")).toHaveLength(0);

    batch.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await settle();
    expect(fake.callsFor("agent_updates_cancel_all")).toHaveLength(1);
    expect(fake.callsFor("agent_update_answer")).toHaveLength(0);
    expect(store.prompt).toEqual(PROMPT);
  });

  it("Escape from a focused cancel control still answers No, and prompt-level Enter does too", async () => {
    fake.resolve("agent_update_answer", true);
    seedPass();
    setAgentUpdateStore("prompt", PROMPT);
    const button = byTestId<HTMLButtonElement>("agent-update.node.a.cancel");
    button.focus();

    const escape = keydownOn(button, "Escape");
    expect(escape.defaultPrevented).toBe(true);
    await settle();
    expect(fake.lastCall("agent_update_answer")?.args).toEqual({ command: "claude", enabled: false });
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(0);

    // a prompt-level Enter (not from a cancel control) still answers No
    resetAgentUpdateForTests();
    fake.clearCalls();
    seedPass();
    setAgentUpdateStore("prompt", PROMPT);
    const enter = keydownOn(byTestId("agent-update.prompt.no"), "Enter");
    expect(enter.defaultPrevented).toBe(true);
    await settle();
    expect(fake.lastCall("agent_update_answer")?.args).toEqual({ command: "claude", enabled: false });
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(0);
  });

  // -------------------------------------------------------------------------
  // response folding as the user sees it
  // -------------------------------------------------------------------------

  it("response-before-event keeps the row disabled with no cancellation event at all", async () => {
    fake.resolve("agent_update_cancel", { command: "a", disposition: "requested" });
    resolveHydration();
    seedPass();

    click("agent-update.node.a.cancel");
    await settle();

    // no `agent_update_cancellation_changed` is ever emitted
    const button = byTestId<HTMLButtonElement>("agent-update.node.a.cancel");
    expect(button.disabled).toBe(true);
    expect(byTestId("agent-update.node.a.state").textContent).toBe("Cancelling...");
    // a second click cannot reissue
    click("agent-update.node.a.cancel");
    await settle();
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(1);
  });

  it("an all-alreadyTerminal batch response disables the batch and every row control", async () => {
    resolveHydration();
    fake.resolve("agent_updates_cancel_all", {
      requested: [],
      alreadyRequested: [],
      alreadyTerminal: [ref("a", "A"), ref("b", "B")],
    });
    seedPass();

    click("agent-update.cancel-all");
    await settle();

    expect(store.cancelAllRequested).toBe(true);
    expect(byTestId<HTMLButtonElement>("agent-update.cancel-all").disabled).toBe(true);
    for (const button of cancelButtons()) expect(button.disabled).toBe(true);
    // no result was fabricated: the rows are still nonterminal
    expect(store.results).toEqual([]);
    expect(cancelButtons()).toHaveLength(3);
  });

  it("an accepted row response whose hydration fails keeps the row disabled, toasts nothing and never reissues", async () => {
    fake.resolve("agent_update_cancel", { command: "a", disposition: "already_terminal" });
    fake.reject("get_agent_update_status", "boom");
    seedPass();

    click("agent-update.node.a.cancel");
    await settle();

    expect(byTestId<HTMLButtonElement>("agent-update.node.a.cancel").disabled).toBe(true);
    expect(toastStore.items).toHaveLength(0);
    expect(errorSpy).toHaveBeenCalledTimes(1);
    // no backend text leaked into the DOM
    expect(byTestId("agent-update.overlay").textContent).not.toContain("boom");

    click("agent-update.node.a.cancel");
    await settle();
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(1);
  });

  it("an accepted batch response whose hydration fails keeps everything disabled, toasts nothing and never reissues", async () => {
    fake.resolve("agent_updates_cancel_all", {
      requested: [ref("a", "A")],
      alreadyRequested: [],
      alreadyTerminal: [],
    });
    fake.reject("get_agent_update_status", "boom");
    seedPass();

    click("agent-update.cancel-all");
    await settle();

    expect(byTestId<HTMLButtonElement>("agent-update.cancel-all").disabled).toBe(true);
    for (const button of cancelButtons()) expect(button.disabled).toBe(true);
    expect(toastStore.items).toHaveLength(0);
    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(byTestId("agent-update.overlay").textContent).not.toContain("boom");

    click("agent-update.cancel-all");
    await settle();
    expect(fake.callsFor("agent_updates_cancel_all")).toHaveLength(1);
  });

  it("a row invoke rejection shows the exact fixed toast, appends no backend text and permits one retry", async () => {
    fake.reject("agent_update_cancel", "transport closed");
    seedPass();

    click("agent-update.node.a.cancel");
    await settle();

    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("error");
    expect(toastStore.items[0].message).toBe("Could not cancel the coding agent update.");
    expect(toastStore.items[0].message).not.toContain("transport closed");
    // the row is actionable again
    const button = byTestId<HTMLButtonElement>("agent-update.node.a.cancel");
    expect(button.disabled).toBe(false);
    expect(byTestId("agent-update.node.a.state").textContent).toBe("Updating...");

    fake.resolve("agent_update_cancel", { command: "a", disposition: "requested" });
    resolveHydration();
    click("agent-update.node.a.cancel");
    await settle();
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(2);
    expect(byTestId<HTMLButtonElement>("agent-update.node.a.cancel").disabled).toBe(true);
  });

  it("a batch invoke rejection shows the exact fixed toast and permits one retry", async () => {
    fake.reject("agent_updates_cancel_all", "transport closed");
    seedPass();

    click("agent-update.cancel-all");
    await settle();

    expect(toastStore.items.map((item) => item.message)).toEqual([
      "Could not cancel coding agent updates.",
    ]);
    expect(toastStore.items[0].message).not.toContain("transport closed");
    const batch = byTestId<HTMLButtonElement>("agent-update.cancel-all");
    expect(batch.disabled).toBe(false);
    for (const button of cancelButtons()) expect(button.disabled).toBe(false);

    fake.resolve("agent_updates_cancel_all", {
      requested: [ref("a", "A")],
      alreadyRequested: [],
      alreadyTerminal: [],
    });
    resolveHydration();
    click("agent-update.cancel-all");
    await settle();
    expect(fake.callsFor("agent_updates_cancel_all")).toHaveLength(2);
    expect(byTestId<HTMLButtonElement>("agent-update.cancel-all").disabled).toBe(true);
  });

  it("a successful batch response keeps every row action disabled until terminal settlement", async () => {
    resolveHydration();
    fake.resolve("agent_updates_cancel_all", {
      requested: [ref("a", "A")],
      alreadyRequested: [],
      alreadyTerminal: [],
    });
    seedPass();

    click("agent-update.cancel-all");
    await settle();
    for (const button of cancelButtons()) expect(button.disabled).toBe(true);

    // a row that settles simply loses its control; the others stay disabled
    setAgentUpdateStore({ running: [], results: [cancelled("a", "A")] });
    expect(query("agent-update.node.a.cancel")).toBeNull();
    for (const button of cancelButtons()) expect(button.disabled).toBe(true);
  });

  it("a remount re-renders the row and batch controls already disabled by the store latches", async () => {
    resolveHydration();
    fake.resolve("agent_updates_cancel_all", {
      requested: [ref("a", "A")],
      alreadyRequested: [],
      alreadyTerminal: [],
    });
    fake.resolve("agent_update_cancel", { command: "b", disposition: "already_terminal" });
    seedPass();

    click("agent-update.node.b.cancel");
    click("agent-update.cancel-all");
    await settle();

    // the surface goes away and comes back; the shared store carries both latches
    dispose?.();
    dispose = null;
    document.body.replaceChildren();
    mount();

    expect(byTestId<HTMLButtonElement>("agent-update.cancel-all").disabled).toBe(true);
    expect(byTestId<HTMLButtonElement>("agent-update.node.b.cancel").disabled).toBe(true);
    expect(byTestId("agent-update.node.b.state").textContent).toBe("Cancelling...");
    for (const button of cancelButtons()) expect(button.disabled).toBe(true);

    // and unmounting never invoked a cancellation of its own
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(1);
    expect(fake.callsFor("agent_updates_cancel_all")).toHaveLength(1);
  });

  it("a live cancellation event disables the row on a surface that never clicked anything", async () => {
    unlisteners.push(...(await (async () => {
      fake.resolve("get_agent_update_status", null);
      return wireAgentUpdateListeners();
    })()));
    fake.emitFromBackend("agent_updates_started", {
      nodes: [node("a", "A"), node("b", "B")],
    });
    fake.emitFromBackend("agent_update_command_started", node("a", "A"));
    expect(byTestId<HTMLButtonElement>("agent-update.node.a.cancel").disabled).toBe(false);

    fake.emitFromBackend("agent_update_cancellation_changed", {
      cancelRequested: [ref("a", "A")],
      cancelAllRequested: true,
    });
    expect(byTestId<HTMLButtonElement>("agent-update.node.a.cancel").disabled).toBe(true);
    expect(byTestId("agent-update.node.a.state").textContent).toBe("Cancelling...");
    expect(byTestId<HTMLButtonElement>("agent-update.cancel-all").disabled).toBe(true);

    // the terminal cancellation removes the control and prints the outcome
    fake.emitFromBackend("agent_update_command_finished", cancelled("a", "A"));
    expect(query("agent-update.node.a.cancel")).toBeNull();
    expect(byTestId("agent-update.node.a.detail").textContent).toBe("Cancelled");
    expect(toastStore.items).toHaveLength(0);
  });
});
