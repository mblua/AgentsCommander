// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../shared/testing/fake-transport";
import { __setTransportForTests } from "../shared/ipc";
import { toastStore } from "../shared/stores/toasts";
import type {
  AgentUpdateCommandRef,
  AgentUpdateNode,
  AgentUpdatePrompt,
  AgentUpdateResult,
  AgentUpdateResultWire,
  AgentUpdateStatus,
  InstallState,
} from "../shared/types";
import {
  BATCH_CANCEL_FAILED_TOAST,
  ROW_CANCEL_FAILED_TOAST,
  agentUpdateInitialState,
  agentUpdateStore,
  cancelAgentUpdateRow,
  cancelAllAgentUpdates,
  dismissAgentUpdateSummary,
  markPromptClosed,
  mergeSnapshot,
  normalizeAgentUpdateResult,
  resetAgentUpdateForTests,
  wireAgentUpdateListeners,
} from "./agent-update";
import { deriveTimelineNodes } from "./agent-update-status";

const [store, setAgentUpdateStore] = agentUpdateStore;

const FAILING: AgentUpdateResult = {
  command: "claude",
  label: "Claude",
  ok: false,
  outcome: "failed",
  error: "exit code 1",
  installBefore: null,
  installAfter: null,
  change: "unknown",
};

// Frozen: a Solid path write (`set("prompt", next)`) merges `next` INTO the object the store
// already holds, so an unfrozen shared constant would be mutated by the next prompt event.
const PROMPT: AgentUpdatePrompt = Object.freeze({ command: "claude", label: "Claude" });
const CODEX_PROMPT: AgentUpdatePrompt = Object.freeze({ command: "codex", label: "Codex" });

function node(command: string, installBefore?: InstallState): AgentUpdateNode {
  const label = command.charAt(0).toUpperCase() + command.slice(1);
  return installBefore
    ? { command, label, updateCommands: [`${command} update`], installBefore }
    : { command, label, updateCommands: [`${command} update`] };
}

function label(command: string): string {
  return command.charAt(0).toUpperCase() + command.slice(1);
}

function ref(command: string): AgentUpdateCommandRef {
  return { command, label: label(command) };
}

/** #1691 - the canonical succeeded result of a #1691 backend. */
function ok(command: string): AgentUpdateResult {
  return {
    command,
    label: label(command),
    ok: true,
    outcome: "succeeded",
    installBefore: null,
    installAfter: null,
    change: "unknown",
  };
}

function failed(command: string, error = "exit code 1"): AgentUpdateResult {
  return { ...ok(command), ok: false, outcome: "failed", error };
}

/** #1691 - `ok=false` and NEVER a failure: this result must emit no `Auto-update failed` toast. */
function cancelled(command: string): AgentUpdateResult {
  return { ...ok(command), ok: false, outcome: "cancelled" };
}

/** #1691 - the pre-#1691 wire shape: no `outcome`, no probes, no `change`. */
function legacyOk(command: string): AgentUpdateResultWire {
  return { command, label: label(command), ok: true };
}

function legacyFailed(command: string, error = "exit code 1"): AgentUpdateResultWire {
  return { command, label: label(command), ok: false, error };
}

function installed(version: string, seq: number): InstallState {
  return { status: "installed", version, path: `C:\\bin\\${version}.cmd`, seq };
}

function status(overrides: Partial<AgentUpdateStatus> = {}): AgentUpdateStatus {
  return {
    inProgress: false,
    prompt: null,
    results: [],
    running: [],
    verifying: [],
    cancelRequested: [],
    cancelAllRequested: false,
    ...overrides,
  };
}

/** Apply a snapshot through the pure merge exactly as the wiring does. */
function applySnapshot(snapshot: AgentUpdateStatus): void {
  setAgentUpdateStore(mergeSnapshot(store, snapshot));
}

/** One macrotask: every pending microtask (the getStatus chain included) has run. */
async function settle(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("wireAgentUpdateListeners (#1327)", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    resetAgentUpdateForTests();
  });

  afterEach(() => {
    toastStore.clear();
    resetAgentUpdateForTests();
    restore();
    vi.restoreAllMocks();
  });

  it("subscribe-then-snapshot dedups: event + snapshot with the same failing command toast exactly once", async () => {
    fake.resolve("get_agent_update_status", status({ results: [FAILING] }));
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_finished", { results: [FAILING] });
    await settle(); // flush the snapshot promise

    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("error");
    expect(toastStore.items[0].message).toBe(
      "Auto-update failed for Claude (claude): exit code 1"
    );
  });

  it("agent_updates_started sets inProgress; agent_updates_finished clears it and closes the prompt", async () => {
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_started", null);
    expect(store.inProgress).toBe(true);

    fake.emitFromBackend("agent_update_prompt", PROMPT);
    expect(store.prompt).toEqual(PROMPT);

    fake.emitFromBackend("agent_updates_finished", { results: [] });
    expect(store.inProgress).toBe(false);
    expect(store.prompt).toBeNull();
  });

  it("agent_update_prompt_closed clears the prompt but keeps the splash (F4)", async () => {
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_started", null);
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });

    expect(store.prompt).toBeNull();
    expect(store.inProgress).toBe(true);
  });

  it("snapshot restores a prompt emitted before wiring (F3) and the inProgress splash", async () => {
    fake.resolve("get_agent_update_status", status({ inProgress: true, prompt: PROMPT }));
    await wireAgentUpdateListeners();
    await settle();

    expect(store.inProgress).toBe(true);
    expect(store.prompt).toEqual(PROMPT);
  });

  it("getStatus failure never breaks the live listeners (F8)", async () => {
    fake.reject("get_agent_update_status", "boom");
    const unlisteners = await wireAgentUpdateListeners();
    expect(unlisteners).toHaveLength(10);

    fake.emitFromBackend("agent_updates_finished", { results: [FAILING] });
    expect(toastStore.items).toHaveLength(1);
  });
});

describe("agent-update store: per-command events and the monotonic snapshot merge (#1551)", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    resetAgentUpdateForTests();
  });

  afterEach(() => {
    toastStore.clear();
    resetAgentUpdateForTests();
    restore();
    vi.restoreAllMocks();
  });

  it("command_started/finished maintain running and results", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });

    fake.emitFromBackend("agent_update_command_started", node("claude"));
    fake.emitFromBackend("agent_update_command_started", node("codex"));
    // a duplicate started for a running command is ignored
    fake.emitFromBackend("agent_update_command_started", node("claude"));
    expect(store.running).toEqual([
      { command: "claude", label: "Claude" },
      { command: "codex", label: "Codex" },
    ]);

    fake.emitFromBackend("agent_update_command_finished", ok("claude"));
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
    expect(store.results).toEqual([ok("claude")]);

    fake.emitFromBackend("agent_update_command_finished", failed("codex"));
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([ok("claude"), failed("codex")]);
  });

  it("agent_updates_finished clears running even without command_finished", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_update_command_started", node("claude"));
    expect(store.running).toHaveLength(1);

    fake.emitFromBackend("agent_updates_finished", { results: [ok("claude")] });
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([ok("claude")]);
    expect(store.inProgress).toBe(false);
    expect(store.finishedSeen).toBe(true);
  });

  it("snapshot restores running and results", async () => {
    fake.resolve(
      "get_agent_update_status",
      status({
        inProgress: true,
        running: [{ command: "codex", label: "Codex" }],
        results: [ok("claude")],
      })
    );
    await wireAgentUpdateListeners();
    await settle();

    expect(store.inProgress).toBe(true);
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
    expect(store.results).toEqual([ok("claude")]);
    // a mid-pass snapshot toasts nothing (round 6, Grinch R3)
    expect(toastStore.items).toHaveLength(0);
  });

  it("snapshot unions running with newer command_started events", () => {
    setAgentUpdateStore({ inProgress: true, running: [{ command: "codex", label: "Codex" }] });
    // event first: the snapshot (computed before the mark) reports no running command
    applySnapshot(status({ inProgress: true, running: [] }));
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
  });

  it("snapshot adds running commands the store missed", () => {
    setAgentUpdateStore({ inProgress: true, running: [{ command: "codex", label: "Codex" }] });
    applySnapshot(
      status({
        inProgress: true,
        running: [{ command: "pi", label: "Pi" }, { command: "codex", label: "Codex" }],
      })
    );
    expect(store.running).toEqual([
      { command: "codex", label: "Codex" },
      { command: "pi", label: "Pi" },
    ]);
  });

  it("results in either source remove a command from running (both directions)", () => {
    // the store has the result, the snapshot still reports the command running
    setAgentUpdateStore({ inProgress: true, results: [ok("codex")] });
    applySnapshot(status({ inProgress: true, running: [{ command: "codex", label: "Codex" }] }));
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([ok("codex")]);

    // the store still has the command running, the snapshot carries its result
    resetAgentUpdateForTests();
    setAgentUpdateStore({ inProgress: true, running: [{ command: "codex", label: "Codex" }] });
    applySnapshot(status({ inProgress: true, results: [failed("codex")] }));
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([failed("codex")]);
  });

  it("a snapshot after agent_updates_finished never resurrects inProgress, running, or prompt", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });
    fake.emitFromBackend("agent_updates_finished", { results: [ok("claude")] });

    applySnapshot(
      status({
        inProgress: true,
        running: [{ command: "codex", label: "Codex" }],
        prompt: PROMPT,
        results: [ok("claude"), failed("codex")],
      })
    );
    expect(store.inProgress).toBe(false);
    expect(store.running).toEqual([]);
    expect(store.prompt).toBeNull();
    expect(store.results).toEqual([ok("claude"), failed("codex")]);
  });

  it("agent_updates_started resets running, results, finishedSeen, closedPrompts and skippedNodes", async () => {
    await wireAgentUpdateListeners();
    setAgentUpdateStore({
      running: [{ command: "codex", label: "Codex" }],
      results: [ok("claude")],
      finishedSeen: true,
      closedPrompts: ["claude"],
      skippedNodes: ["pi"],
      prompt: CODEX_PROMPT,
    });

    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    expect(store.inProgress).toBe(true);
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([]);
    expect(store.finishedSeen).toBe(false);
    expect(store.closedPrompts).toEqual([]);
    expect(store.skippedNodes).toEqual([]);
    expect(store.nodes).toEqual([node("claude")]);
    // the prompt is untouched by started
    expect(store.prompt).toEqual(CODEX_PROMPT);
  });

  it("a snapshot without running (older backend) merges as empty", () => {
    setAgentUpdateStore({ inProgress: true, running: [{ command: "codex", label: "Codex" }] });
    const older = { inProgress: true, prompt: null, results: [] } as unknown as AgentUpdateStatus;
    applySnapshot(older);
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
    expect(store.nodes).toEqual([]);
  });

  it("prompt_closed carries the command: an older snapshot cannot resurrect that prompt", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });
    expect(store.prompt).toBeNull();
    expect(store.closedPrompts).toContain("claude");

    applySnapshot(status({ inProgress: true, prompt: PROMPT }));
    expect(store.prompt).toBeNull();
  });

  it("a new prompt after a closure still appears from an event and from a snapshot", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });

    fake.emitFromBackend("agent_update_prompt", CODEX_PROMPT);
    expect(store.prompt).toEqual(CODEX_PROMPT);

    setAgentUpdateStore("prompt", null);
    applySnapshot(status({ inProgress: true, prompt: CODEX_PROMPT }));
    expect(store.prompt).toEqual(CODEX_PROMPT);
    expect(store.closedPrompts).toEqual(["claude"]);
  });

  it("a null prompt_closed payload (older backend) still clears the prompt", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", null);
    expect(store.prompt).toBeNull();
    expect(store.closedPrompts).toEqual([]);
  });

  it("a delayed prompt_closed(A) does not clear a newer prompt B", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt", CODEX_PROMPT);
    expect(store.prompt).toEqual(CODEX_PROMPT);

    fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });
    expect(store.prompt?.command).toBe("codex");
    expect(store.closedPrompts).toEqual(["claude"]);

    fake.emitFromBackend("agent_update_prompt_closed", { command: "codex", label: "Codex" });
    expect(store.prompt).toBeNull();
    expect(store.closedPrompts).toEqual(["claude", "codex"]);
  });

  it("markPromptClosed protects the prompt the overlay answered itself", () => {
    setAgentUpdateStore({ inProgress: true, prompt: null });
    markPromptClosed("claude");
    markPromptClosed("claude");
    expect(store.closedPrompts).toEqual(["claude"]);

    applySnapshot(status({ inProgress: true, prompt: PROMPT }));
    expect(store.prompt).toBeNull();
  });

  it("agent_updates_started carries the pass nodes in order and resets installAfter and summary", async () => {
    await wireAgentUpdateListeners();
    setAgentUpdateStore({ installAfter: { claude: installed("1.0", 4) }, summary: "shown" });

    fake.emitFromBackend("agent_updates_started", {
      nodes: [node("claude"), node("pi"), node("codex")],
    });
    expect(store.nodes.map((n) => n.command)).toEqual(["claude", "pi", "codex"]);
    expect(store.installAfter).toEqual({});
    expect(store.summary).toBe("none");
  });

  it("a null started payload (older backend) leaves nodes empty", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", null);
    expect(store.inProgress).toBe(true);
    expect(store.nodes).toEqual([]);
  });

  it("command_skipped removes exactly that node", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", {
      nodes: [node("claude"), node("codex"), node("pi")],
    });

    fake.emitFromBackend("agent_update_command_skipped", { command: "codex", label: "Codex" });
    expect(store.nodes.map((n) => n.command)).toEqual(["claude", "pi"]);
    expect(store.skippedNodes).toEqual(["codex"]);
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([]);
  });

  it("command_started records installBefore on its node and appends to running", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });

    const before = installed("1.0", 0);
    fake.emitFromBackend("agent_update_command_started", node("codex", before));
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
    expect(store.nodes.map((n) => n.command)).toEqual(["claude", "codex"]);
    expect(store.nodes[1].installBefore).toEqual(before);
    expect(store.nodes[0].installBefore).toBeUndefined();
  });

  it("command_started for a command absent from nodes appends a defensive node", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", null);

    fake.emitFromBackend("agent_update_command_started", node("codex"));
    expect(store.nodes).toEqual([node("codex")]);
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
  });

  it("install_state_changed is kept by max seq for a pass node, before or after its result, and ignored for commands outside the pass", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", {
      nodes: [node("claude", installed("1.0", 0)), node("codex", installed("2.0", 0))],
    });

    // an event for a pass node BEFORE its command_finished is kept (FE N2)
    fake.emitFromBackend("agent_update_command_started", node("codex"));
    fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: installed("2.1", 1) });
    expect(store.installAfter.codex).toEqual(installed("2.1", 1));
    const codexView = () =>
      deriveTimelineNodes(store.nodes, store.running, store.verifying, store.results).find(
        (view) => view.command === "codex"
      );
    // ...the running node shows no outcome yet...
    expect(codexView()).toMatchObject({ state: "updating", detail: null });
    fake.emitFromBackend("agent_update_command_finished", {
      ...ok("codex"),
      change: "changed",
      installBefore: installed("2.0", 0),
      installAfter: installed("2.1", 1),
    });
    expect(store.installAfter.codex).toEqual(installed("2.1", 1));
    // #1691 - the row's text comes from the RESULT's own probes, never from this cache
    expect(codexView()).toMatchObject({ state: "ok", detail: "Ready - 2.0 -> 2.1" });
    fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: installed("9.9", 8) });
    expect(store.installAfter.codex).toEqual(installed("9.9", 8));
    expect(codexView()).toMatchObject({ state: "ok", detail: "Ready - 2.0 -> 2.1" });

    // after the result: seq 3 is kept, a later seq 2 is ignored
    fake.emitFromBackend("agent_update_command_finished", ok("claude"));
    fake.emitFromBackend("agent_install_state_changed", { command: "claude", install: installed("1.1", 3) });
    expect(store.installAfter.claude).toEqual(installed("1.1", 3));
    fake.emitFromBackend("agent_install_state_changed", { command: "claude", install: installed("0.9", 2) });
    expect(store.installAfter.claude).toEqual(installed("1.1", 3));

    // a command outside the pass never enters installAfter
    fake.emitFromBackend("agent_install_state_changed", { command: "pi", install: installed("5.0", 9) });
    expect(store.installAfter).toEqual({ codex: installed("9.9", 8), claude: installed("1.1", 3) });

    // a newer state REPLACES the entry: keys the new state omits (version, path) do not survive
    const gone: InstallState = { status: "missing", detail: "'claude' was not found on PATH", seq: 4 };
    fake.emitFromBackend("agent_install_state_changed", { command: "claude", install: gone });
    expect(store.installAfter.claude).toEqual(gone);
    expect(store.installAfter.claude).not.toHaveProperty("version");
    expect(store.installAfter.claude).not.toHaveProperty("path");
  });

  it("finished enters the summary only when the pass was shown and results are non-empty", async () => {
    await wireAgentUpdateListeners();

    // shown + results -> shown and no toast yet
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_updates_finished", { results: [failed("claude")] });
    expect(store.summary).toBe("shown");
    expect(store.inProgress).toBe(false);
    expect(toastStore.items).toHaveLength(0);

    // not shown -> none and toasts at once
    resetAgentUpdateForTests();
    toastStore.clear();
    fake.emitFromBackend("agent_updates_finished", { results: [failed("claude")] });
    expect(store.summary).toBe("none");
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("Auto-update failed for Claude (claude): exit code 1");

    // shown + empty results -> none
    resetAgentUpdateForTests();
    toastStore.clear();
    fake.emitFromBackend("agent_updates_started", { nodes: [] });
    fake.emitFromBackend("agent_updates_finished", { results: [] });
    expect(store.summary).toBe("none");
    expect(toastStore.items).toHaveLength(0);
  });

  it("dismissAgentUpdateSummary toasts each failure once and marks dismissed", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });
    fake.emitFromBackend("agent_updates_finished", {
      results: [failed("claude"), failed("codex", "timed out after 300s (killed)")],
    });
    expect(store.summary).toBe("shown");
    expect(toastStore.items).toHaveLength(0);

    dismissAgentUpdateSummary();
    expect(store.summary).toBe("dismissed");
    expect(toastStore.items.map((item) => item.kind)).toEqual(["error", "error"]);
    expect(toastStore.items.map((item) => item.message)).toEqual([
      "Auto-update failed for Claude (claude): exit code 1",
      "Auto-update failed for Codex (codex): timed out after 300s (killed)",
    ]);

    dismissAgentUpdateSummary();
    expect(store.summary).toBe("dismissed");
    expect(toastStore.items).toHaveLength(2);
  });

  it("a snapshot seeds nodes when the store has none and never resurrects a skipped node", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("a"), node("b")] });
    fake.emitFromBackend("agent_update_command_skipped", { command: "b", label: "B" });
    setAgentUpdateStore({ installAfter: { a: installed("1.1", 2) }, summary: "shown" });

    applySnapshot(status({ inProgress: true, nodes: [node("a"), node("b")] }));
    expect(store.nodes.map((n) => n.command)).toEqual(["a"]);
    // a snapshot never changes summary or installAfter
    expect(store.summary).toBe("shown");
    expect(store.installAfter).toEqual({ a: installed("1.1", 2) });

    resetAgentUpdateForTests();
    applySnapshot(status({ inProgress: true, nodes: [node("a"), node("b")] }));
    expect(store.nodes.map((n) => n.command)).toEqual(["a", "b"]);
    expect(store.summary).toBe("none");
    expect(store.installAfter).toEqual({});
  });

  it("a snapshot computed before the pass started never shrinks the nodes", () => {
    const started = { ...agentUpdateInitialState(), inProgress: true, nodes: [node("a"), node("b")] };

    const prePass = mergeSnapshot(started, status({ inProgress: false, nodes: [], running: [], results: [] }));
    expect(prePass.nodes.map((n) => n.command)).toEqual(["a", "b"]);

    const inPass = mergeSnapshot(started, status({ inProgress: true, nodes: [node("a")] }));
    expect(inPass.nodes.map((n) => n.command)).toEqual(["a"]);

    const seeded = mergeSnapshot(
      agentUpdateInitialState(),
      status({ inProgress: false, nodes: [node("a"), node("b")] })
    );
    expect(seeded.nodes.map((n) => n.command)).toEqual(["a", "b"]);
  });

  it("a command_skipped seen before an older snapshot never resurrects the node", async () => {
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_update_command_skipped", { command: "b", label: "B" });
    expect(store.skippedNodes).toEqual(["b"]);
    expect(store.nodes).toEqual([]);

    applySnapshot(status({ inProgress: true, nodes: [node("a"), node("b")] }));
    expect(store.nodes.map((n) => n.command)).toEqual(["a"]);

    setAgentUpdateStore("nodes", []);
    applySnapshot(status({ inProgress: false, nodes: [node("a"), node("b")] }));
    expect(store.nodes.map((n) => n.command)).toEqual(["a"]);

    fake.emitFromBackend("agent_updates_started", { nodes: [node("a"), node("b")] });
    expect(store.skippedNodes).toEqual([]);
    expect(store.nodes.map((n) => n.command)).toEqual(["a", "b"]);
  });

  it("a mid-pass snapshot defers the failure toasts to the summary close, a post-pass snapshot toasts at once", async () => {
    // (i) a surface mounted in the middle of the pass
    fake.resolve(
      "get_agent_update_status",
      status({
        inProgress: true,
        nodes: [node("a"), node("b")],
        running: [{ command: "b", label: "B" }],
        results: [failed("a")],
        answered: {},
      })
    );
    await wireAgentUpdateListeners();
    await settle();
    expect(store.inProgress).toBe(true);
    expect(store.results).toEqual([failed("a")]);
    expect(toastStore.items).toHaveLength(0);

    fake.emitFromBackend("agent_updates_finished", { results: [failed("a"), ok("b")] });
    expect(store.summary).toBe("shown");
    expect(toastStore.items).toHaveLength(0);

    dismissAgentUpdateSummary();
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("Auto-update failed for A (a): exit code 1");

    // (ii) a surface mounted after the pass
    resetAgentUpdateForTests();
    toastStore.clear();
    fake.resolve("get_agent_update_status", status({ inProgress: false, results: [failed("a")] }));
    await wireAgentUpdateListeners();
    await settle();
    expect(toastStore.items).toHaveLength(1);
    expect(store.summary).toBe("none");
  });
});

describe("#1691 - verification, cancellation folds and terminal first-winner", () => {
  let fake: FakeTransport;
  let restore: () => void;
  let errorSpy: ReturnType<typeof vi.spyOn>;

  /** The row cancel/batch/status triple a happy-path test needs. */
  function resolveCancel(disposition: string): void {
    fake.resolve("agent_update_cancel", { command: "claude", disposition });
  }

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    resetAgentUpdateForTests();
    errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    toastStore.clear();
    resetAgentUpdateForTests();
    restore();
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // normalization
  // -------------------------------------------------------------------------

  it("normalizeAgentUpdateResult applies exactly the four documented inferences", () => {
    expect(normalizeAgentUpdateResult(legacyOk("a"))).toEqual({
      command: "a",
      label: "A",
      ok: true,
      outcome: "succeeded",
      installBefore: null,
      installAfter: null,
      change: "unknown",
    });
    expect(normalizeAgentUpdateResult(legacyFailed("b", "boom"))).toEqual({
      command: "b",
      label: "B",
      ok: false,
      error: "boom",
      outcome: "failed",
      installBefore: null,
      installAfter: null,
      change: "unknown",
    });
    // a canonical result is returned unchanged, cancelled included
    expect(normalizeAgentUpdateResult(cancelled("c"))).toEqual(cancelled("c"));
    // a present outcome always wins over the `ok` inference
    expect(normalizeAgentUpdateResult({ ...legacyOk("d"), ok: false, outcome: "cancelled" })).toMatchObject({
      outcome: "cancelled",
    });
    // an absent verification diagnostic stays absent, never null
    expect(normalizeAgentUpdateResult(legacyOk("e"))).not.toHaveProperty("verificationError");
  });

  // -------------------------------------------------------------------------
  // cancelled results never toast the legacy failure copy
  // -------------------------------------------------------------------------

  it("a cancelled result emits no failure toast on a dismissed summary, and a genuine failure still does", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });
    fake.emitFromBackend("agent_updates_finished", { results: [cancelled("claude"), failed("codex")] });
    expect(store.summary).toBe("shown");
    expect(toastStore.items).toHaveLength(0);

    dismissAgentUpdateSummary();
    expect(toastStore.items.map((item) => item.message)).toEqual([
      "Auto-update failed for Codex (codex): exit code 1",
    ]);
  });

  it("a cancelled result emits no failure toast on a late/no-summary finished surface", async () => {
    await wireAgentUpdateListeners();
    // no `agent_updates_started` on this surface: the immediate-toast path
    fake.emitFromBackend("agent_updates_finished", { results: [cancelled("claude"), failed("codex")] });
    expect(store.summary).toBe("none");
    expect(toastStore.items.map((item) => item.message)).toEqual([
      "Auto-update failed for Codex (codex): exit code 1",
    ]);
  });

  it("listener-first getStatus() on an already-finished pass toasts the failure only, never the cancellation", async () => {
    fake.resolve(
      "get_agent_update_status",
      status({ inProgress: false, results: [cancelled("claude"), failed("codex")] })
    );
    await wireAgentUpdateListeners();
    await settle();
    expect(toastStore.items.map((item) => item.message)).toEqual([
      "Auto-update failed for Codex (codex): exit code 1",
    ]);
  });

  it("a LEGACY cancelled result normalizes to `failed` and does toast: only a truthful outcome suppresses it", async () => {
    // The guarantee is about the canonical contract: an older backend that cannot say
    // `cancelled` is indistinguishable from a failure, and the normalization is exact.
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_finished", { results: [legacyFailed("claude")] });
    expect(store.results[0].outcome).toBe("failed");
    expect(toastStore.items).toHaveLength(1);

    // a legacy SUCCESS never toasts
    resetAgentUpdateForTests();
    toastStore.clear();
    fake.emitFromBackend("agent_updates_finished", { results: [legacyOk("claude")] });
    expect(store.results[0]).toEqual(ok("claude"));
    expect(toastStore.items).toHaveLength(0);
  });

  it("an absent probe and a false `ok` are never failure predicates on their own", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_finished", {
      results: [{ ...ok("claude"), installAfter: null, change: "unknown" }, cancelled("codex")],
    });
    expect(store.results.map((result) => result.ok)).toEqual([true, false]);
    expect(toastStore.items).toHaveLength(0);
  });

  it("an empty final payload still enters the summary when the store already holds results", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_update_command_finished", failed("claude"));
    expect(store.results).toEqual([failed("claude")]);

    // This state is NOT reachable in production: the backend's finalize sweeps every
    // target into the published result before it announces, so an empty `results` can
    // never accompany a pass this surface saw produce one. It is constructed directly
    // to pin the fold, which is the single input on which the two readings differ:
    // `showSummary` tests the MERGED results, not the payload's. Testing the payload's
    // length instead would send this surface down the immediate-toast path with an
    // empty list, so the failure it already observed would never be toasted at all.
    fake.emitFromBackend("agent_updates_finished", { results: [] });
    expect(store.results).toEqual([failed("claude")]);
    expect(store.summary).toBe("shown");
    expect(toastStore.items).toHaveLength(0);

    dismissAgentUpdateSummary();
    expect(toastStore.items.map((item) => item.message)).toEqual([
      "Auto-update failed for Claude (claude): exit code 1",
    ]);
  });

  // -------------------------------------------------------------------------
  // verifying
  // -------------------------------------------------------------------------

  it("agent_update_command_verifying moves a row out of running and keeps it unfinished and cancellable", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });
    fake.emitFromBackend("agent_update_command_started", node("claude"));
    expect(store.running).toEqual([ref("claude")]);

    fake.emitFromBackend("agent_update_command_verifying", ref("claude"));
    expect(store.running).toEqual([]);
    expect(store.verifying).toEqual([ref("claude")]);

    const view = () =>
      deriveTimelineNodes(store.nodes, store.running, store.verifying, store.results).find(
        (entry) => entry.command === "claude"
      );
    // unfinished, not done, not failed, and it still offers the row action
    expect(view()).toMatchObject({ state: "verifying", stateText: "Verifying...", terminal: false, cancellable: true });

    // a stale `started` never pulls a verifying row backwards
    fake.emitFromBackend("agent_update_command_started", node("claude"));
    expect(store.running).toEqual([]);
    expect(store.verifying).toEqual([ref("claude")]);

    // only the terminal result ends it
    fake.emitFromBackend("agent_update_command_finished", ok("claude"));
    expect(store.verifying).toEqual([]);
    expect(view()).toMatchObject({ terminal: true, cancellable: false });
  });

  it("a verifying row that was requested renders Cancelling... and stays uncancellable until terminal", async () => {
    fake.resolve("get_agent_update_status", status());
    resolveCancel("requested");
    await wireAgentUpdateListeners();
    await settle();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_update_command_verifying", ref("claude"));

    await cancelAgentUpdateRow("claude");
    expect(store.cancelRequested).toEqual([ref("claude")]);
    expect(store.verifying).toEqual([ref("claude")]);

    const cancelling = new Set(store.cancelRequested.map((entry) => entry.command));
    const view = deriveTimelineNodes(store.nodes, store.running, store.verifying, store.results, cancelling)[0];
    expect(view).toMatchObject({ state: "cancelling", stateText: "Cancelling...", cancellable: false, terminal: false });

    // the terminal cancellation is the first winner and clears every collection
    fake.emitFromBackend("agent_update_command_finished", cancelled("claude"));
    expect(store.verifying).toEqual([]);
    expect(store.cancelRequested).toEqual([]);
    expect(store.cancelResponses).toEqual([]);
    expect(store.results).toEqual([cancelled("claude")]);
  });

  // -------------------------------------------------------------------------
  // terminal first winner
  // -------------------------------------------------------------------------

  it("start, verifying, cancellation, final and stale snapshots never overwrite the first terminal result", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });
    fake.emitFromBackend("agent_update_command_finished", ok("claude"));
    expect(store.results).toEqual([ok("claude")]);

    // a second result for the same command is dropped
    fake.emitFromBackend("agent_update_command_finished", failed("claude", "late"));
    expect(store.results).toEqual([ok("claude")]);

    // start / verifying / cancellation cannot resurrect it
    fake.emitFromBackend("agent_update_command_started", node("claude"));
    fake.emitFromBackend("agent_update_command_verifying", ref("claude"));
    fake.emitFromBackend("agent_update_cancellation_changed", {
      cancelRequested: [ref("claude")],
      cancelAllRequested: false,
    });
    expect(store.running).toEqual([]);
    expect(store.verifying).toEqual([]);
    expect(store.cancelRequested).toEqual([]);

    // a stale snapshot filters the terminal row out of every in-progress array
    applySnapshot(
      status({
        inProgress: true,
        running: [ref("claude")],
        verifying: [ref("claude")],
        cancelRequested: [ref("claude")],
        results: [failed("claude", "stale")],
      })
    );
    expect(store.results).toEqual([ok("claude")]);
    expect(store.running).toEqual([]);
    expect(store.verifying).toEqual([]);
    expect(store.cancelRequested).toEqual([]);

    // the FINAL payload merges missing commands only
    fake.emitFromBackend("agent_updates_finished", {
      results: [failed("claude", "final"), ok("codex")],
    });
    expect(store.results).toEqual([ok("claude"), ok("codex")]);
  });

  it("a verifying event or snapshot never clears an already-observed cancellation request", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_update_cancellation_changed", {
      cancelRequested: [ref("claude")],
      cancelAllRequested: false,
    });
    fake.emitFromBackend("agent_update_command_verifying", ref("claude"));
    expect(store.cancelRequested).toEqual([ref("claude")]);

    applySnapshot(status({ inProgress: true, verifying: [ref("claude")], cancelRequested: [] }));
    expect(store.cancelRequested).toEqual([ref("claude")]);
    expect(store.verifying).toEqual([ref("claude")]);
  });

  it("cancelAllRequested only ever ORs upward within a pass, and only a new pass resets it", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_update_cancellation_changed", {
      cancelRequested: [],
      cancelAllRequested: true,
    });
    expect(store.cancelAllRequested).toBe(true);

    // a delayed false event, and a delayed false snapshot, are both inert
    fake.emitFromBackend("agent_update_cancellation_changed", {
      cancelRequested: [],
      cancelAllRequested: false,
    });
    expect(store.cancelAllRequested).toBe(true);
    applySnapshot(status({ inProgress: true, cancelAllRequested: false }));
    expect(store.cancelAllRequested).toBe(true);

    // it survives the finished boundary...
    fake.emitFromBackend("agent_updates_finished", { results: [ok("claude")] });
    expect(store.cancelAllRequested).toBe(true);
    applySnapshot(status({ cancelAllRequested: false }));
    expect(store.cancelAllRequested).toBe(true);

    // ...and only the next pass clears it
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    expect(store.cancelAllRequested).toBe(false);
  });

  it("agent_updates_started resets verifying, cancelRequested, cancelAllRequested and both latches", async () => {
    await wireAgentUpdateListeners();
    setAgentUpdateStore({
      verifying: [ref("codex")],
      cancelRequested: [ref("codex")],
      cancelAllRequested: true,
      cancelResponses: ["codex"],
    });

    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    expect(store.verifying).toEqual([]);
    expect(store.cancelRequested).toEqual([]);
    expect(store.cancelAllRequested).toBe(false);
    expect(store.cancelResponses).toEqual([]);
  });

  // -------------------------------------------------------------------------
  // row response folding
  // -------------------------------------------------------------------------

  it("`requested` and `already_requested` latch the row AND fold it into cancelRequested, then hydrate once", async () => {
    for (const disposition of ["requested", "already_requested"]) {
      resetAgentUpdateForTests();
      fake.clearCalls();
      fake.resolve("get_agent_update_status", status({ inProgress: true, nodes: [node("claude")] }));
      resolveCancel(disposition);
      setAgentUpdateStore({ inProgress: true, nodes: [node("claude")], running: [ref("claude")] });

      const accepted = await cancelAgentUpdateRow("claude");
      expect(accepted).toBe(true);
      expect(fake.lastCall("agent_update_cancel")?.args).toEqual({ command: "claude" });
      expect(store.cancelResponses).toEqual(["claude"]);
      expect(store.cancelRequested).toEqual([ref("claude")]);
      expect(fake.callsFor("get_agent_update_status")).toHaveLength(1);
      expect(toastStore.items).toHaveLength(0);
    }
  });

  it("`already_terminal` and `not_in_pass` latch the row, fabricate no result and request nothing", async () => {
    for (const disposition of ["already_terminal", "not_in_pass"]) {
      resetAgentUpdateForTests();
      fake.clearCalls();
      fake.resolve("get_agent_update_status", status({ inProgress: true, nodes: [node("claude")] }));
      resolveCancel(disposition);
      setAgentUpdateStore({ inProgress: true, nodes: [node("claude")], running: [ref("claude")] });

      const accepted = await cancelAgentUpdateRow("claude");
      expect(accepted).toBe(true);
      expect(store.cancelResponses).toEqual(["claude"]);
      expect(store.cancelRequested).toEqual([]);
      expect(store.results).toEqual([]);
      expect(toastStore.items).toHaveLength(0);
    }
  });

  it("the row latch survives a missing event, a delayed false snapshot and a later hydration, and only the terminal result clears it", async () => {
    fake.resolve("get_agent_update_status", status({ inProgress: true }));
    resolveCancel("requested");
    setAgentUpdateStore({ inProgress: true, nodes: [node("claude")], running: [ref("claude")] });

    await cancelAgentUpdateRow("claude");
    expect(store.cancelResponses).toEqual(["claude"]);

    // no cancellation event ever arrives, and a snapshot reports no request at all
    applySnapshot(status({ inProgress: true, running: [ref("claude")], cancelRequested: [] }));
    expect(store.cancelResponses).toEqual(["claude"]);
    expect(store.cancelRequested).toEqual([ref("claude")]);

    // the terminal result is the only thing that clears both
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_command_finished", cancelled("claude"));
    expect(store.cancelResponses).toEqual([]);
    expect(store.cancelRequested).toEqual([]);
  });

  it("hydration rejection after ANY accepted row response keeps the latch, logs only, and toasts nothing", async () => {
    for (const disposition of ["requested", "already_requested", "already_terminal", "not_in_pass"]) {
      resetAgentUpdateForTests();
      toastStore.clear();
      errorSpy.mockClear();
      fake.clearCalls();
      fake.reject("get_agent_update_status", "boom");
      resolveCancel(disposition);
      setAgentUpdateStore({ inProgress: true, nodes: [node("claude")], running: [ref("claude")] });

      const accepted = await cancelAgentUpdateRow("claude");
      expect(accepted).toBe(true);
      // latched despite the failed refresh
      expect(store.cancelResponses).toEqual(["claude"]);
      // neither cancellation-failure string, and no raw backend text anywhere
      expect(toastStore.items).toHaveLength(0);
      // diagnostic to console.error only, and exactly one cancel invoke
      expect(errorSpy).toHaveBeenCalledTimes(1);
      expect(String(errorSpy.mock.calls[0][0])).toContain("[agent-update] getStatus after cancel failed:");
      expect(fake.callsFor("agent_update_cancel")).toHaveLength(1);
    }
  });

  it("only an invoke/backend rejection BEFORE a response toasts the exact row string and permits one retry", async () => {
    fake.reject("agent_update_cancel", "transport closed");
    setAgentUpdateStore({ inProgress: true, nodes: [node("claude")], running: [ref("claude")] });

    const accepted = await cancelAgentUpdateRow("claude");
    expect(accepted).toBe(false);
    expect(store.cancelResponses).toEqual([]);
    expect(store.cancelRequested).toEqual([]);
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("error");
    expect(toastStore.items[0].message).toBe("Could not cancel the coding agent update.");
    expect(ROW_CANCEL_FAILED_TOAST).toBe("Could not cancel the coding agent update.");
    // no raw backend text is appended to the fixed copy
    expect(toastStore.items[0].message).not.toContain("transport closed");
    expect(errorSpy).toHaveBeenCalledTimes(1);
    // no hydration was attempted, and the row may be retried
    expect(fake.callsFor("get_agent_update_status")).toHaveLength(0);

    fake.resolve("get_agent_update_status", status({ inProgress: true }));
    resolveCancel("requested");
    expect(await cancelAgentUpdateRow("claude")).toBe(true);
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(2);
    expect(store.cancelResponses).toEqual(["claude"]);
  });

  it("row cancellation preserves completed peers and never resurrects their terminal state", async () => {
    fake.resolve("get_agent_update_status", status({ inProgress: true, running: [ref("codex")] }));
    resolveCancel("requested");
    setAgentUpdateStore({
      inProgress: true,
      nodes: [node("claude"), node("codex")],
      running: [ref("claude"), ref("codex")],
      results: [ok("codex")],
    });

    await cancelAgentUpdateRow("claude");
    expect(store.results).toEqual([ok("codex")]);
    expect(store.cancelRequested).toEqual([ref("claude")]);
    expect(store.cancelResponses).toEqual(["claude"]);
    // the completed peer stays out of every in-progress collection
    expect(store.running.map((entry) => entry.command)).not.toContain("codex");
  });

  // -------------------------------------------------------------------------
  // batch response folding
  // -------------------------------------------------------------------------

  it("a batch response latches cancelAllRequested and folds requested + alreadyRequested only", async () => {
    fake.resolve("get_agent_update_status", status({ inProgress: true }));
    fake.resolve("agent_updates_cancel_all", {
      requested: [ref("claude")],
      alreadyRequested: [ref("codex")],
      alreadyTerminal: [ref("pi")],
    });
    setAgentUpdateStore({ inProgress: true, nodes: [node("claude"), node("codex"), node("pi")] });

    expect(await cancelAllAgentUpdates()).toBe(true);
    expect(fake.lastCall("agent_updates_cancel_all")?.args).toEqual({});
    expect(store.cancelAllRequested).toBe(true);
    expect(store.cancelRequested).toEqual([ref("claude"), ref("codex")]);
    expect(store.cancelResponses).toEqual(["claude", "codex"]);
    // alreadyTerminal fabricates no result and no request
    expect(store.results).toEqual([]);
    expect(fake.callsFor("get_agent_update_status")).toHaveLength(1);
  });

  it("an all-alreadyTerminal batch response still latches the batch", async () => {
    fake.resolve("get_agent_update_status", status({ inProgress: true }));
    fake.resolve("agent_updates_cancel_all", {
      requested: [],
      alreadyRequested: [],
      alreadyTerminal: [ref("claude"), ref("codex")],
    });
    setAgentUpdateStore({ inProgress: true, nodes: [node("claude"), node("codex")] });

    expect(await cancelAllAgentUpdates()).toBe(true);
    expect(store.cancelAllRequested).toBe(true);
    expect(store.cancelRequested).toEqual([]);
    expect(store.results).toEqual([]);
    expect(toastStore.items).toHaveLength(0);
  });

  it("an only-alreadyRequested batch response latches the batch too", async () => {
    fake.resolve("get_agent_update_status", status({ inProgress: true }));
    fake.resolve("agent_updates_cancel_all", {
      requested: [],
      alreadyRequested: [ref("claude")],
      alreadyTerminal: [],
    });
    setAgentUpdateStore({ inProgress: true, nodes: [node("claude")] });

    expect(await cancelAllAgentUpdates()).toBe(true);
    expect(store.cancelAllRequested).toBe(true);
    expect(store.cancelRequested).toEqual([ref("claude")]);
  });

  it("hydration rejection after ANY accepted batch response keeps the latch, logs only, and toasts nothing", async () => {
    const responses = [
      { requested: [ref("claude")], alreadyRequested: [], alreadyTerminal: [] },
      { requested: [], alreadyRequested: [ref("claude")], alreadyTerminal: [] },
      { requested: [], alreadyRequested: [], alreadyTerminal: [ref("claude")] },
    ];
    for (const response of responses) {
      resetAgentUpdateForTests();
      toastStore.clear();
      errorSpy.mockClear();
      fake.clearCalls();
      fake.reject("get_agent_update_status", "boom");
      fake.resolve("agent_updates_cancel_all", response);
      setAgentUpdateStore({ inProgress: true, nodes: [node("claude")] });

      expect(await cancelAllAgentUpdates()).toBe(true);
      expect(store.cancelAllRequested).toBe(true);
      expect(toastStore.items).toHaveLength(0);
      expect(errorSpy).toHaveBeenCalledTimes(1);
      expect(String(errorSpy.mock.calls[0][0])).toContain("[agent-update] getStatus after cancel failed:");
      expect(fake.callsFor("agent_updates_cancel_all")).toHaveLength(1);
    }
  });

  it("only an invoke/backend rejection BEFORE a batch response toasts the exact batch string and permits one retry", async () => {
    fake.reject("agent_updates_cancel_all", "transport closed");
    setAgentUpdateStore({ inProgress: true, nodes: [node("claude")] });

    expect(await cancelAllAgentUpdates()).toBe(false);
    expect(store.cancelAllRequested).toBe(false);
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("Could not cancel coding agent updates.");
    expect(BATCH_CANCEL_FAILED_TOAST).toBe("Could not cancel coding agent updates.");
    expect(toastStore.items[0].message).not.toContain("transport closed");
    expect(fake.callsFor("get_agent_update_status")).toHaveLength(0);

    fake.resolve("get_agent_update_status", status({ inProgress: true }));
    fake.resolve("agent_updates_cancel_all", { requested: [ref("claude")], alreadyRequested: [], alreadyTerminal: [] });
    expect(await cancelAllAgentUpdates()).toBe(true);
    expect(fake.callsFor("agent_updates_cancel_all")).toHaveLength(2);
    expect(store.cancelAllRequested).toBe(true);
  });

  // -------------------------------------------------------------------------
  // response/event/snapshot permutations
  // -------------------------------------------------------------------------

  it("event-before-response and response-before-event both end latched, and neither double-cancels", async () => {
    // (i) event first
    fake.resolve("get_agent_update_status", status({ inProgress: true }));
    resolveCancel("already_requested");
    await wireAgentUpdateListeners();
    await settle();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_update_cancellation_changed", {
      cancelRequested: [ref("claude")],
      cancelAllRequested: false,
    });
    await cancelAgentUpdateRow("claude");
    expect(store.cancelRequested).toEqual([ref("claude")]);
    expect(store.cancelResponses).toEqual(["claude"]);

    // (ii) response first, event later
    resetAgentUpdateForTests();
    fake.clearCalls();
    resolveCancel("requested");
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    await cancelAgentUpdateRow("claude");
    expect(store.cancelResponses).toEqual(["claude"]);
    fake.emitFromBackend("agent_update_cancellation_changed", {
      cancelRequested: [ref("claude")],
      cancelAllRequested: true,
    });
    expect(store.cancelRequested).toEqual([ref("claude")]);
    expect(store.cancelAllRequested).toBe(true);
    expect(fake.callsFor("agent_update_cancel")).toHaveLength(1);
  });

  it("a full listener-first hydration restores cancellation, verifying, both probes, change and a terminal cancellation", async () => {
    const restored: AgentUpdateResult = {
      ...ok("pi"),
      change: "changed",
      installBefore: installed("1.0", 0),
      installAfter: installed("1.1", 1),
    };
    fake.resolve(
      "get_agent_update_status",
      status({
        inProgress: true,
        nodes: [node("claude"), node("codex"), node("pi"), node("hermes")],
        running: [ref("claude")],
        verifying: [ref("codex")],
        cancelRequested: [ref("claude"), ref("codex")],
        cancelAllRequested: true,
        results: [restored, cancelled("hermes")],
      })
    );
    await wireAgentUpdateListeners();
    await settle();

    expect(store.running).toEqual([ref("claude")]);
    expect(store.verifying).toEqual([ref("codex")]);
    expect(store.cancelRequested).toEqual([ref("claude"), ref("codex")]);
    expect(store.cancelAllRequested).toBe(true);
    expect(store.results).toEqual([restored, cancelled("hermes")]);

    const cancelling = new Set(store.cancelRequested.map((entry) => entry.command));
    const views = deriveTimelineNodes(store.nodes, store.running, store.verifying, store.results, cancelling);
    expect(views.map((view) => view.state)).toEqual(["cancelling", "cancelling", "ok", "cancelled"]);
    expect(views.map((view) => view.detail)).toEqual([
      null,
      null,
      "Ready - 1.0 -> 1.1",
      "Cancelled",
    ]);
    // a mid-pass hydration toasts nothing
    expect(toastStore.items).toHaveLength(0);
  });

  it("a remount (fresh listeners over the surviving store) keeps both latches and issues no cancellation", async () => {
    fake.resolve("get_agent_update_status", status({ inProgress: true, nodes: [node("claude")] }));
    resolveCancel("requested");
    setAgentUpdateStore({ inProgress: true, nodes: [node("claude")], running: [ref("claude")] });
    await cancelAgentUpdateRow("claude");
    await cancelAllAgentUpdatesWith(fake);
    expect(store.cancelResponses).toEqual(["claude"]);
    expect(store.cancelAllRequested).toBe(true);

    // the surface remounts: the store is shared, so the latches survive
    const unlisteners = await wireAgentUpdateListeners();
    await settle();
    expect(store.cancelResponses).toEqual(["claude"]);
    expect(store.cancelAllRequested).toBe(true);

    // teardown unlistens ONLY: no cancel command is ever invoked by cleanup
    const before = fake.callsFor("agent_update_cancel").length + fake.callsFor("agent_updates_cancel_all").length;
    for (const unlisten of unlisteners) unlisten();
    expect(fake.callsFor("agent_update_cancel").length + fake.callsFor("agent_updates_cancel_all").length).toBe(before);
  });

  /** Batch-cancel with a response the caller does not care about; keeps the remount test short. */
  async function cancelAllAgentUpdatesWith(transport: FakeTransport): Promise<void> {
    transport.resolve("agent_updates_cancel_all", {
      requested: [ref("claude")],
      alreadyRequested: [],
      alreadyTerminal: [],
    });
    await cancelAllAgentUpdates();
  }
});
