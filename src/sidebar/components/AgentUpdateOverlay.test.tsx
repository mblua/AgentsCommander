// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import { toastStore } from "../../shared/stores/toasts";
import type { UnlistenFn } from "../../shared/transport";
import type {
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

function ok(command: string, label: string): AgentUpdateResult {
  return { command, label, ok: true };
}

function failed(command: string, label: string, error: string): AgentUpdateResult {
  return { command, label, ok: false, error };
}

function status(overrides: Partial<AgentUpdateStatus> = {}): AgentUpdateStatus {
  return { inProgress: true, prompt: null, results: [], running: [], ...overrides };
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
    expect(overlay.textContent).toContain("Actualizando coding agents...");
    expect(overlay.getAttribute("data-ac-state")).toBe("pass");

    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });
    expect(overlay.textContent).toContain("coding agent Claude");
    expect(overlay.getAttribute("data-ac-state")).toBe("prompt");
    expect(byTestId("agent-update.prompt.yes").textContent).toBe("Sí");
    expect(byTestId("agent-update.prompt.no").textContent).toBe("No");
  });

  it("Sí answer applied this boot closes the modal, reads no snapshot and runs nothing else", async () => {
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

  it("late Sí answer closes the modal AND toasts the pinned conditional info text", async () => {
    fake.resolve("agent_update_answer", false);
    fake.resolve("get_agent_update_status", status({ answered: { claude: true } }));
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.yes");
    await settle();

    expect(store.prompt).toBeNull();
    expect(fake.callsFor("get_agent_update_status")).toHaveLength(1);
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("info");
    expect(toastStore.items[0].message).toBe("Se actualizará en el próximo arranque.");
  });

  it("late No answer closes the modal AND toasts the conditional No text", async () => {
    fake.resolve("agent_update_answer", false);
    fake.resolve("get_agent_update_status", status({ answered: { claude: false } }));
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    click("agent-update.prompt.no");
    await settle();

    expect(store.prompt).toBeNull();
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("No se volverá a preguntar.");
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
    expect(byTestId("agent-update.timeline").getAttribute("aria-label")).toBe("Agentes de la actualización");
    expect(byTestId("agent-update.timeline").getAttribute("data-ac-role")).toBe("list");

    const items = nodeItems();
    expect(items.map((li) => li.getAttribute("data-ac-testid"))).toEqual([
      "agent-update.node.a",
      "agent-update.node.b",
      "agent-update.node.c",
    ]);
    expect(items.map((li) => li.getAttribute("data-ac-state"))).toEqual(["ok", "updating", "pending"]);
    expect(items.map((li) => li.getAttribute("data-ac-command"))).toEqual(["a", "b", "c"]);
    expect(["a", "b", "c"].map((command) => byTestId(`agent-update.node.${command}.state`).textContent)).toEqual([
      "Listo",
      "Actualizando...",
      "Pendiente",
    ]);
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

  it("finished nodes show the version transition and the inline error on ONE line", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("a", "A", installed("1.0", 0)), node("b", "B", installed("1.0", 0))],
      results: [ok("a", "A"), failed("b", "B", "exit code 1")],
      installAfter: { a: installed("1.1", 2), b: missing(3) },
    });
    expect(byTestId("agent-update.node.a.detail").textContent).toBe("1.0 → 1.1");
    expect(byTestId("agent-update.node.a.detail").getAttribute("title")).toBe("1.0 → 1.1");
    expect(byTestId("agent-update.node.a").querySelector(".agent-update-node-line")!.textContent).toBe(
      "Listo · 1.0 → 1.1"
    );

    const item = byTestId("agent-update.node.b");
    expect(item.getAttribute("data-ac-state")).toBe("failed");
    expect(item.querySelector(".agent-update-node-marker svg")).toBeTruthy();
    const detail = byTestId("agent-update.node.b.detail");
    expect(detail.textContent).toBe("exit code 1 · 1.0 → no instalada");
    expect(detail.getAttribute("title")).toBe("exit code 1 · 1.0 → no instalada");
    // the whole node is ONE line: state, a visible aria-hidden separator, and the detail
    const lines = item.querySelectorAll(".agent-update-node-line");
    expect(lines).toHaveLength(1);
    expect(lines[0].textContent).toBe("Falló · exit code 1 · 1.0 → no instalada");
    expect(byTestId("agent-update.node.b.state").textContent).toBe("Falló");
    const separator = lines[0].querySelector(".agent-update-node-sep")!;
    expect(separator.textContent).toBe(" · ");
    expect(separator.getAttribute("aria-hidden")).toBe("true");
    expect(item.querySelector(".agent-update-node-head")).toBeNull();
  });

  it("the header counter, attributes and progressbar follow the counts", () => {
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("a", "A"), node("b", "B"), node("c", "C")],
      results: [ok("a", "A")],
    });
    const text = byTestId("agent-update.progress.text");
    expect(text.textContent).toBe("1 de 3 completados");
    expect(text.getAttribute("data-ac-done")).toBe("1");
    expect(text.getAttribute("data-ac-total")).toBe("3");
    expect(text.getAttribute("data-ac-failed")).toBe("0");
    const bar = byTestId("agent-update.progress");
    expect(bar.getAttribute("role")).toBe("progressbar");
    expect(bar.getAttribute("aria-label")).toBe("Agentes completados");
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuenow")).toBe("1");
    expect(bar.getAttribute("aria-valuemax")).toBe("3");
    expect(bar.querySelector<HTMLElement>(".agent-update-progress-fill")!.style.width).toBe("33%");

    setAgentUpdateStore("results", [ok("a", "A"), failed("b", "B", "exit code 1")]);
    expect(text.textContent).toBe("2 de 3 completados · 1 falló");
    expect(text.getAttribute("data-ac-failed")).toBe("1");
    expect(bar.getAttribute("aria-valuenow")).toBe("2");
    expect(bar.querySelector<HTMLElement>(".agent-update-progress-fill")!.style.width).toBe("67%");
  });

  it("no timeline and no counter during the prompt branch, none with zero nodes", () => {
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT, nodes: [node("a", "A")] });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("prompt");
    expect(query("agent-update.timeline")).toBeNull();
    expect(query("agent-update.progress.text")).toBeNull();
    expect(query("agent-update.progress")).toBeNull();
    expect(query("agent-update.header")).toBeNull();
    expect(document.querySelector(".agent-update-card--timeline")).toBeNull();
    expect(byTestId("agent-update.prompt.no")).toBeTruthy();

    resetAgentUpdateForTests();
    setAgentUpdateStore({ inProgress: true });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("pass");
    expect(document.querySelector(".agent-update-spinner")).toBeTruthy();
    expect(byTestId("agent-update.title").textContent).toBe("Actualizando coding agents...");
    expect(query("agent-update.timeline")).toBeNull();
    expect(query("agent-update.progress.text")).toBeNull();
    expect(query("agent-update.progress")).toBeNull();
    expect(query("agent-update.summary.close")).toBeNull();
  });

  it("finished enters the summary and it persists until Cerrar", async () => {
    vi.useFakeTimers();
    await wire();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("a", "A"), node("b", "B")] });
    fake.emitFromBackend("agent_update_command_finished", ok("a", "A"));
    fake.emitFromBackend("agent_update_command_finished", failed("b", "B", "exit code 1"));
    fake.emitFromBackend("agent_updates_finished", { results: [ok("a", "A"), failed("b", "B", "exit code 1")] });

    const overlay = byTestId("agent-update.overlay");
    expect(overlay.getAttribute("data-ac-state")).toBe("summary");
    expect(byTestId("agent-update.title").textContent).toBe("Coding agents actualizados");
    expect(byTestId("agent-update.done").querySelector("svg")).toBeTruthy();
    expect(byTestId("agent-update.done").getAttribute("aria-hidden")).toBe("true");
    expect(document.querySelector(".agent-update-spinner")).toBeNull();
    expect(byTestId("agent-update.progress.text").textContent).toBe("2 de 2 completados · 1 falló");
    expect(byTestId("agent-update.progress").getAttribute("aria-valuenow")).toBe("2");
    expect(nodeItems().map((li) => li.getAttribute("data-ac-state"))).toEqual(["ok", "failed"]);
    const close = byTestId<HTMLButtonElement>("agent-update.summary.close");
    expect(close.textContent).toBe("Cerrar");
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
    expect(byTestId("agent-update.node.a.state").textContent).toBe("Pendiente");
  });

  it("the failure toast appears on close, not on finished", async () => {
    await wire();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("bad-stub", "Bad")] });
    fake.emitFromBackend("agent_updates_finished", { results: [failed("bad-stub", "Bad", "exit code 1")] });
    expect(store.summary).toBe("shown");
    expect(toastStore.items).toHaveLength(0);
    expect(byTestId("agent-update.node.bad-stub").querySelector(".agent-update-node-line")!.textContent).toBe(
      "Falló · exit code 1"
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

  it("install_state_changed after finished updates the detail in place", async () => {
    await wire();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("a", "A", installed("1.0", 0))] });
    fake.emitFromBackend("agent_updates_finished", { results: [ok("a", "A")] });
    expect(byTestId("agent-update.overlay").getAttribute("data-ac-state")).toBe("summary");
    const detail = byTestId("agent-update.node.a.detail");
    expect(detail.textContent).toBe("1.0");

    fake.emitFromBackend("agent_install_state_changed", { command: "a", install: installed("1.1", 1) });
    expect(byTestId("agent-update.node.a.detail")).toBe(detail); // the <li> is stable (<Index>)
    expect(detail.textContent).toBe("1.0 → 1.1");
    expect(detail.getAttribute("title")).toBe("1.0 → 1.1");
  });

  it("the title carries role=status and aria-live=polite", () => {
    setAgentUpdateStore({ inProgress: true });
    const title = byTestId("agent-update.title");
    expect(title.getAttribute("role")).toBe("status");
    expect(title.getAttribute("aria-live")).toBe("polite");
    expect(title.classList.contains("agent-update-text")).toBe(true);
  });

  it("the Pendiente state word keeps the AA token while the label stays dimmed", () => {
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
    expect(toastStore.items[0].message).toBe("No se volverá a preguntar.");
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
    expect(byTestId("agent-update.overlay").textContent).toContain("coding agent Codex");

    pending.resolve(true);
    await settle();

    expect(store.prompt).toEqual({ command: "codex", label: "Codex" });
    expect(byTestId("agent-update.overlay").textContent).toContain("coding agent Codex");
    expect(byTestId<HTMLButtonElement>("agent-update.prompt.yes").disabled).toBe(false);
    expect(byTestId<HTMLButtonElement>("agent-update.prompt.no").disabled).toBe(false);
    expect(store.closedPrompts).toContain("claude");
    expect(store.closedPrompts).not.toContain("codex");
    expect(toastStore.items).toHaveLength(0);
    expect(fake.callsFor("agent_update_answer")).toHaveLength(1);
  });
});
